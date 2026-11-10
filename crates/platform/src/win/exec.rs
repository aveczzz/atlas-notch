//! Executa os `Command` que os módulos pedem.
//!
//! - `FocusEditor` (feito): `EnumWindows`, ficar com as janelas de editores (VS Code e
//!   derivados) e escolher a que tem o projeto como segmento do título (o VS Code mostra
//!   `ficheiro - pasta - Visual Studio Code`). `SetForegroundWindow` funciona porque o clique
//!   que pediu isto foi o último input do utilizador, e foi para a nossa janela.
//! - Media (feito): vai para a thread `media`, que chama `Try*Async` na sessão GSMTC atual
//!   (fire-and-forget, o resultado chega pelos eventos).
//! - Volume (fase 6): `IAudioEndpointVolume::SetMasterVolumeLevelScalar`,
//!   `ISimpleAudioVolume::SetMasterVolume` (com o nosso `event_context`).
//! - `DismissNotification`: vai para a thread `notifications` (`RemoveNotification(id)`).
//!
//! Nada aqui pode bloquear a thread UI: operações lentas vão para a thread da fonte
//! respetiva através de um canal.

use super::audio::{AudioCommand, AudioControl};
use super::media::{MediaCommand, MediaControl};
use super::notifications::NotificationControl;
use super::process;
use island_core::Command;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOWNORMAL,
    SetForegroundWindow, ShowWindow,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::core::{BOOL, HSTRING, PCWSTR, w};

/// Executáveis de editores com o mesmo formato de título que o VS Code.
const EDITORS: &[&str] = &["code.exe", "code - insiders.exe", "cursor.exe", "windsurf.exe", "vscodium.exe"];

/// Para onde vão os comandos que precisam de uma fonte (cada uma na sua thread).
#[derive(Debug, Default)]
pub struct Sinks {
    pub media: Option<MediaControl>,
    pub audio: Option<AudioControl>,
    pub notifications: Option<NotificationControl>,
}

pub fn run(commands: impl Iterator<Item = Command>, sinks: &Sinks) {
    for cmd in commands {
        match cmd {
            Command::FocusEditor { project } => focus_editor(project.as_deref()),
            Command::MediaPlayPause | Command::MediaNext | Command::MediaPrevious => {
                let media_cmd = match cmd {
                    Command::MediaNext => MediaCommand::Next,
                    Command::MediaPrevious => MediaCommand::Previous,
                    _ => MediaCommand::PlayPause,
                };
                match &sinks.media {
                    Some(m) => m.send(media_cmd),
                    None => log!("comando sem fonte de media: {cmd:?}"),
                }
            }
            Command::SetMasterVolume(_) | Command::SetSessionVolume { .. } | Command::SetSessionMute { .. } => {
                let audio_cmd = match cmd {
                    Command::SetMasterVolume(v) => AudioCommand::Master(v),
                    Command::SetSessionVolume { id, volume } => AudioCommand::SessionVolume { id, volume },
                    Command::SetSessionMute { id, muted } => AudioCommand::SessionMute { id, muted },
                    _ => unreachable!("filtrado pelo padrão acima"),
                };
                match &sinks.audio {
                    Some(a) => a.send(audio_cmd),
                    None => log!("comando sem fonte de áudio: {audio_cmd:?}"),
                }
            }
            Command::DismissNotification { id } => match &sinks.notifications {
                Some(n) => n.dismiss(id),
                None => log!("comando sem fonte de notificações: {cmd:?}"),
            },
            Command::OpenFile { path } => open_file(&path),
            Command::OpenApp { aumid } => open_app(&aumid),
        }
    }
}

/// Abre um ficheiro da Tray com a app predefinida, como um duplo clique no Explorador.
/// Funciona em primeiro plano porque o clique que o pediu foi para a nossa janela.
fn open_file(path: &str) {
    // SAFETY: strings locais vivas durante a chamada; sem janela-mãe.
    let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    // O ShellExecute devolve um "HINSTANCE" > 32 em sucesso (convenção antiga do Win16).
    if result.0 as usize <= 32 {
        log!("tray: não consegui abrir {path} (código {})", result.0 as usize);
    }
}

/// Abre uma app pelo AppUserModelId (`shell:AppsFolder<aumid>`), como o menu Iniciar.
fn open_app(aumid: &str) {
    let target = HSTRING::from(format!(r"shell:AppsFolder\{aumid}"));
    // SAFETY: strings locais vivas durante a chamada; sem janela-mãe.
    let result = unsafe { ShellExecuteW(None, w!("open"), &target, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if result.0 as usize <= 32 {
        log!("notificações: não consegui abrir {aumid} (código {})", result.0 as usize);
    }
}

fn focus_editor(project: Option<&str>) {
    let windows = editor_windows();
    let titles: Vec<&str> = windows.iter().map(|(_, t)| t.as_str()).collect();
    let Some(i) = pick_window(&titles, project) else {
        log!("abrir no editor: nenhuma janela do VS Code para {project:?}");
        return;
    };
    let hwnd = windows[i].0;
    // SAFETY: `hwnd` veio do `EnumWindows` agora mesmo; se entretanto fechou, as chamadas
    // falham sem efeito.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            log!("abrir no editor: o Windows recusou trazer a janela para a frente");
        }
    }
}

/// Que janela abrir. Primeiro a que tem o projeto como segmento exato do título; depois a
/// que o contém; sem projeto (ou sem correspondência), nenhuma: abrir o projeto errado é
/// pior do que não fazer nada.
fn pick_window(titles: &[&str], project: Option<&str>) -> Option<usize> {
    let project = project?.trim();
    if project.is_empty() {
        return None;
    }
    let segment = |t: &str| t.split(" - ").any(|s| s.trim().trim_start_matches('●').trim().eq_ignore_ascii_case(project));
    titles
        .iter()
        .position(|t| segment(t))
        .or_else(|| titles.iter().position(|t| t.to_lowercase().contains(&project.to_lowercase())))
}

/// Janelas visíveis de editores: (`HWND`, título).
fn editor_windows() -> Vec<(HWND, String)> {
    let mut found: Vec<(HWND, String)> = Vec::new();
    // SAFETY: `found` vive durante o `EnumWindows`, que chama o callback de forma síncrona
    // nesta thread; o `LPARAM` é um ponteiro para ele.
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut _ as isize));
    }
    found
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: o `LPARAM` é o `&mut Vec` passado em `editor_windows`, vivo durante a enumeração.
    let found = unsafe { &mut *(lparam.0 as *mut Vec<(HWND, String)>) };
    // SAFETY: `hwnd` vem do sistema durante a enumeração; buffers locais.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf);
        if len <= 0 {
            return true.into();
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if is_editor(pid) {
            found.push((hwnd, String::from_utf16_lossy(&buf[..len as usize])));
        }
    }
    true.into()
}

fn is_editor(pid: u32) -> bool {
    process::image_path(pid).is_some_and(|path| EDITORS.contains(&process::exe_name(&path.to_lowercase())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TITLES: &[&str] = &[
        "main.rs - island - Visual Studio Code",
        "● README.md - atlas-island - Visual Studio Code",
        "activesync - Visual Studio Code",
    ];

    #[test]
    fn exact_title_segment_wins() {
        assert_eq!(pick_window(TITLES, Some("atlas-island")), Some(1));
        assert_eq!(pick_window(TITLES, Some("island")), Some(0), "não confunde com atlas-island");
        assert_eq!(pick_window(TITLES, Some("ActiveSync")), Some(2), "sem diferença de maiúsculas");
    }

    #[test]
    fn no_project_or_no_match_opens_nothing() {
        assert_eq!(pick_window(TITLES, None), None);
        assert_eq!(pick_window(TITLES, Some("outro")), None);
        assert_eq!(pick_window(&[], Some("island")), None);
    }

    #[test]
    fn partial_match_as_fallback() {
        let titles = ["x.rs - [WSL] island-dev - Visual Studio Code"];
        assert_eq!(pick_window(&titles, Some("island-dev")), Some(0));
    }
}
