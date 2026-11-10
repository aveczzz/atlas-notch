//! Esconder a island quando uma app ocupa o ecrã inteiro (jogos, vídeos, apresentações).
//!
//! Sem polling: a janela regista-se como *appbar* (`ABM_NEW`) **sem reservar espaço**
//! (nunca chama `ABM_SETPOS`). O shell avisa então as appbars com `ABN_FULLSCREENAPP`
//! sempre que uma app entra ou sai de ecrã inteiro, e é o mesmo aviso que a barra de
//! tarefas usa para se esconder.
//!
//! O aviso não diz em que monitor aconteceu. Por isso, a cada aviso, vê-se se a janela
//! em primeiro plano cobre o monitor da island. Um vídeo em ecrã inteiro num monitor
//! externo não esconde a island do portátil.

use super::window::{Input, push};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::Shell::{ABM_NEW, ABM_REMOVE, ABN_FULLSCREENAPP, APPBARDATA, SHAppBarMessage};
use std::cell::Cell;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, GetClassNameW, GetForegroundWindow, GetWindowRect, IsZoomed, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_APP,
};

/// Mensagem com que o shell avisa a appbar.
pub const WM_APP_APPBAR: u32 = WM_APP + 2;

fn data(hwnd: HWND) -> APPBARDATA {
    APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, uCallbackMessage: WM_APP_APPBAR, ..Default::default() }
}

/// Começa a receber os avisos. Se falhar, a island fica sempre visível: não é grave.
pub fn register(hwnd: HWND) {
    let mut d = data(hwnd);
    // SAFETY: `d` é local e tem `cbSize` certo; `hwnd` é a nossa janela.
    if unsafe { SHAppBarMessage(ABM_NEW, &mut d) } == 0 {
        log!("aviso de ecrã inteiro indisponível");
    }
    // O aviso do shell só chega a certas transições, e uma perdida (um ecrã de boas-vindas no
    // arranque do Windows) deixava a island escondida para sempre. A cada mudança de janela em
    // primeiro plano (evento, sem polling) volta-se a ver o estado.
    OURS.set(hwnd);
    // SAFETY: a função de retorno tem a assinatura certa e só toca em estado desta thread;
    // `WINEVENT_OUTOFCONTEXT` entrega-a na thread que registou, pelo ciclo de mensagens.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if hook.is_invalid() {
        log!("aviso de janela em primeiro plano indisponível");
    }
    HOOK.set(hook);
}

thread_local! {
    static OURS: Cell<HWND> = Cell::new(HWND::default());
    static HOOK: Cell<HWINEVENTHOOK> = Cell::new(HWINEVENTHOOK::default());
}

unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    push(Input::Fullscreen(fullscreen_on_our_monitor(OURS.get())));
}

/// Retirar no `WM_DESTROY`, senão o shell guarda uma appbar morta.
pub fn unregister(hwnd: HWND) {
    let mut d = data(hwnd);
    // SAFETY: como em `register`.
    unsafe { SHAppBarMessage(ABM_REMOVE, &mut d) };
    let hook = HOOK.replace(HWINEVENTHOOK::default());
    if !hook.is_invalid() {
        // SAFETY: o gancho registado em `register`, desfeito uma vez.
        let _ = unsafe { UnhookWinEvent(hook) };
    }
}

/// Trata a mensagem `WM_APP_APPBAR`. Só nos interessa `ABN_FULLSCREENAPP`.
pub fn on_message(hwnd: HWND, wparam: WPARAM, _lparam: LPARAM) {
    if wparam.0 as u32 == ABN_FULLSCREENAPP {
        push(Input::Fullscreen(fullscreen_on_our_monitor(hwnd)));
    }
}

/// A janela em primeiro plano ocupa o monitor inteiro onde está a island?
/// Pergunta-se de novo a cada aviso, tanto ao abrir como ao fechar: assim um aviso de
/// outro monitor nunca deixa a island escondida por engano.
fn fullscreen_on_our_monitor(ours: HWND) -> bool {
    // SAFETY: consultas sobre janelas e monitores; todas as estruturas são locais e
    // `cbSize` está preenchido. Uma janela que entretanto fechou só faz as chamadas falhar.
    unsafe {
        let fg = GetForegroundWindow();
        // Uma janela maximizada nunca é ecrã inteiro, mesmo que cubra o monitor (barra de tarefas
        // em esconder automático, janelas sem moldura como o VS Code).
        if fg.is_invalid() || fg == ours || is_desktop(fg) || IsZoomed(fg).as_bool() {
            return false;
        }
        let monitor = MonitorFromWindow(ours, MONITOR_DEFAULTTONULL);
        if monitor.is_invalid() || MonitorFromWindow(fg, MONITOR_DEFAULTTONULL) != monitor {
            return false;
        }
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut rect = RECT::default();
        if !GetMonitorInfoW(monitor, &mut info).as_bool() || GetWindowRect(fg, &mut rect).is_err() {
            return false;
        }
        covers(&rect, &info.rcMonitor)
    }
}

fn covers(window: &RECT, monitor: &RECT) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

/// O ambiente de trabalho (Win+D) também cobre o ecrã e às vezes dispara o aviso.
fn is_desktop(hwnd: HWND) -> bool {
    let mut buf = [0u16; 32];
    // SAFETY: `buf` é local e o tamanho vem do slice.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) }.max(0) as usize;
    let class = String::from_utf16_lossy(&buf[..n]);
    matches!(class.as_str(), "Progman" | "WorkerW")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: RECT = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };

    #[test]
    fn covering_the_whole_monitor_counts_as_fullscreen() {
        assert!(covers(&SCREEN, &SCREEN));
        // Janelas maximizadas "à antiga" passam uns pixeis para fora: também contam.
        assert!(covers(&RECT { left: -8, top: -8, right: 1928, bottom: 1088 }, &SCREEN));
    }

    #[test]
    fn a_maximized_window_above_the_taskbar_does_not() {
        assert!(!covers(&RECT { left: 0, top: 0, right: 1920, bottom: 1032 }, &SCREEN));
    }
}
