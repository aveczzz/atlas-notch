//! Aplicar a configuração no sistema: arranque com o Windows, vigiar o `config.toml`, abri-lo.

use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindCloseChangeNotification,
    FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: PCWSTR = w!("DynamicIsland");

/// A cópia instalada (`%LOCALAPPDATA%\dynamic-island`)? Só essa entra no arranque: uma
/// build de teste em `target\` nunca deve ficar registada.
fn installed_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let home = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("dynamic-island");
    let same = |a: &Path, b: &Path| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
    exe.parent().is_some_and(|dir| same(dir, &home)).then_some(exe)
}

/// Põe ou tira a island do arranque do Windows (`HKCU\...\Run`, só este utilizador).
pub fn apply_autostart(on: bool) {
    let Some(exe) = installed_exe() else { return };
    // SAFETY: chave e valor são literais; `data` vive durante a chamada e o tamanho em
    // bytes inclui o terminador nulo, como o REG_SZ pede.
    let status = unsafe {
        if on {
            let data: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            )
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE) {
                ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                s => s,
            }
        }
    };
    if status != ERROR_SUCCESS {
        log!("arranque com o Windows: erro {}", status.0);
    }
}

/// Avisa quando algo muda na pasta da configuração. O handle entra no
/// `MsgWaitForMultipleObjectsEx` do loop: 0 polling.
#[derive(Debug)]
pub struct Watcher(HANDLE);

impl Watcher {
    pub fn new(dir: &Path) -> Option<Self> {
        // SAFETY: o caminho vive durante a chamada.
        let h = unsafe {
            FindFirstChangeNotificationW(
                &HSTRING::from(dir.as_os_str()),
                false,
                FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME,
            )
        };
        match h {
            Ok(h) if !h.is_invalid() => Some(Self(h)),
            _ => {
                log!("config: não consigo vigiar a pasta; as mudanças só valem ao reiniciar");
                None
            }
        }
    }

    pub fn handle(&self) -> HANDLE {
        self.0
    }

    /// Depois de o handle sinalizar: rearmar para a próxima mudança.
    pub fn rearm(&self) {
        // SAFETY: handle nosso, vivo até ao `Drop`.
        let _ = unsafe { FindNextChangeNotification(self.0) };
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // SAFETY: handle nosso, fechado uma vez.
        let _ = unsafe { FindCloseChangeNotification(self.0) };
    }
}

/// Abre o `config.toml` no editor associado a `.toml`; sem associação, no Bloco de Notas.
pub fn open_config(path: &Path) {
    let file = HSTRING::from(path.as_os_str());
    // SAFETY: strings locais vivas durante as chamadas; sem janela-mãe.
    unsafe {
        let r = ShellExecuteW(None, w!("open"), &file, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        // > 32 é sucesso (convenção do Win16).
        if r.0 as usize <= 32 {
            let quoted = HSTRING::from(format!("\"{}\"", path.display()));
            let r = ShellExecuteW(None, w!("open"), w!("notepad.exe"), &quoted, PCWSTR::null(), SW_SHOWNORMAL);
            if r.0 as usize <= 32 {
                log!("config: não consegui abrir {}", path.display());
            }
        }
    }
}
