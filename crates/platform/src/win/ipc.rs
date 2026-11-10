//! Servidor do named pipe (`island_proto::PIPE_NAME`).
//!
//! Uma thread bloqueante aceita ligações; cada cliente é lido numa thread curta
//! (`headless::read_ipc`), até `MAX_CLIENTS` ao mesmo tempo. Um cliente lento ou parado
//! nunca impede os outros (os hooks do Claude Code chegam aos pares, PreToolUse/PostToolUse).
//!
//! ```text
//! bind()   -> 1.ª instância com FILE_FLAG_FIRST_PIPE_INSTANCE (deteta outra island)
//! loop {
//!   ConnectNamedPipe                  // bloqueia até um cliente ligar
//!   criar já a instância seguinte     // um 2.º hook encontra-a livre, sem ERROR_PIPE_BUSY
//!   thread: read_ipc(cliente)
//! }
//! ```
//!
//! Segurança (não saltar):
//! - `PIPE_REJECT_REMOTE_CLIENTS`: só clientes locais.
//! - DACL explícita só com o SID do utilizador atual (`D:P(A;;GA;;;<SID>)`): outro utilizador
//!   na mesma máquina não injeta mensagens na tua island.
//! - `FILE_FLAG_FIRST_PIPE_INSTANCE` na primeira criação: se o nome já existe (outra island ou
//!   um impostor), `bind` falha e a app sai em vez de correr sem pipe.
//! - Os limites de tamanho e o sanitize estão em `read_ipc` e `island_proto::decode_line`.

use crate::{EventProxy, headless};
use std::fs::File;
use std::io::{self, BufReader};
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{HSTRING, PWSTR};

/// Clientes lidos ao mesmo tempo, no máximo (regra 6). Os seguintes são recusados.
const MAX_CLIENTS: usize = 8;
const BUFFER: u32 = 4096;
/// Erros seguidos ao aceitar ligações antes de desistir (a island continua, sem IPC).
const MAX_ERRORS: u32 = 5;

/// Descritor de segurança com a DACL só do utilizador atual (memória do `LocalAlloc`).
#[derive(Debug)]
struct Security(PSECURITY_DESCRIPTOR);

// SAFETY: o descritor é memória imutável depois de criado; só é lido (passado ao
// `CreateNamedPipeW`) pela thread do servidor, que passa a ser a dona.
unsafe impl Send for Security {}

impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: veio do `ConvertStringSecurityDescriptorToSecurityDescriptorW`, que o
        // aloca com `LocalAlloc`; é libertado uma vez.
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

/// O pipe já reservado, pronto a aceitar clientes.
#[derive(Debug)]
pub struct Server {
    security: Security,
    first: OwnedHandle,
}

/// Cria a primeira instância do pipe. `ErrorKind::AlreadyExists` = já há uma island a correr.
pub fn bind() -> io::Result<Server> {
    let security = current_user_only()?;
    let first = create_instance(&security, true).map_err(|e| {
        if e.raw_os_error() == Some(ERROR_ACCESS_DENIED.0 as i32) {
            io::Error::new(io::ErrorKind::AlreadyExists, "o pipe da island já existe (outra island a correr?)")
        } else {
            e
        }
    })?;
    Ok(Server { security, first })
}

impl Server {
    /// Arranca a thread do servidor.
    pub fn spawn(self, proxy: EventProxy) -> io::Result<()> {
        thread::Builder::new().name("ipc".into()).spawn(move || self.accept_loop(proxy))?;
        Ok(())
    }

    fn accept_loop(self, proxy: EventProxy) {
        let active = Arc::new(AtomicUsize::new(0));
        let mut listening = self.first;
        let mut errors = 0;
        loop {
            let handle = HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&listening));
            // SAFETY: `handle` é a instância que esta thread possui (`listening`), viva até aqui.
            let connected = match unsafe { ConnectNamedPipe(handle, None) } {
                Ok(()) => true,
                // O cliente ligou entre o `CreateNamedPipeW` e o `ConnectNamedPipe`: está ligado.
                Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
            };
            // A instância seguinte já, antes de ler este cliente.
            let next = match create_instance(&self.security, false) {
                Ok(next) => next,
                Err(e) => {
                    log!("ipc: não consegui criar outra instância: {e}");
                    return;
                }
            };
            let client = std::mem::replace(&mut listening, next);
            if !connected {
                errors += 1;
                if errors >= MAX_ERRORS {
                    log!("ipc: demasiados erros seguidos; o pipe fica desligado");
                    return;
                }
                continue;
            }
            errors = 0;
            if active.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
                active.fetch_sub(1, Ordering::AcqRel);
                log!("ipc: demasiados clientes ao mesmo tempo, ligação recusada");
                continue; // `client` fecha aqui.
            }
            let (proxy, counter) = (proxy.clone(), Arc::clone(&active));
            let spawned = thread::Builder::new().name("ipc-client".into()).spawn(move || {
                // Ler até o cliente fechar. O `File` fecha a instância no fim.
                headless::read_ipc(BufReader::new(File::from(client)), &proxy);
                counter.fetch_sub(1, Ordering::AcqRel);
            });
            if spawned.is_err() {
                active.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}

fn create_instance(security: &Security, first: bool) -> io::Result<OwnedHandle> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0.0,
        bInheritHandle: false.into(),
    };
    let mut mode = PIPE_ACCESS_INBOUND;
    if first {
        mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: `attributes` e o descritor que referencia vivem durante a chamada; o handle
    // devolvido passa logo a ser de um `OwnedHandle`, que o fecha.
    unsafe {
        let handle = CreateNamedPipeW(
            &HSTRING::from(island_proto::PIPE_NAME),
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            0,
            BUFFER,
            0,
            Some(&attributes),
        );
        if handle == INVALID_HANDLE_VALUE || handle.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnedHandle::from_raw_handle(handle.0))
    }
}

/// `D:P(A;;GA;;;<SID do utilizador atual>)`: protegida (sem herança), acesso total só para ti.
fn current_user_only() -> io::Result<Security> {
    let sid = current_user_sid()?;
    let sddl = HSTRING::from(format!("D:P(A;;GA;;;{sid})"));
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl` vive durante a chamada; o descritor alocado passa a ser do `Security`.
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut descriptor, None)? };
    Ok(Security(descriptor))
}

fn current_user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: pseudo-handle do processo atual; o token é fechado no fim.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)? };
    let result = (|| {
        let mut len = 0;
        // SAFETY: primeira chamada só pergunta o tamanho (falha com ERROR_INSUFFICIENT_BUFFER).
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
        // `u64` para o buffer ficar alinhado para um `TOKEN_USER`.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` tem pelo menos `len` bytes e está alinhado para `TOKEN_USER`.
        unsafe { GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len)? };
        // SAFETY: a API escreveu um `TOKEN_USER` no início de `buf`; o SID aponta para
        // dentro de `buf`, que vive até ao fim desta closure.
        let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text = PWSTR::null();
        // SAFETY: SID válido (acima); `text` é alocado com `LocalAlloc` e libertado a seguir.
        unsafe {
            ConvertSidToStringSidW(sid, &mut text)?;
            let s = text.to_string();
            LocalFree(Some(HLOCAL(text.0.cast())));
            s.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
    })();
    // SAFETY: token aberto acima, fechado uma vez.
    let _ = unsafe { CloseHandle(token) };
    result
}
