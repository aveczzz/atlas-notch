//! Caps Lock, Num Lock e Scroll Lock.
//!
//! Uma thread própria com uma janela só para mensagens (`HWND_MESSAGE`) e *raw input* do
//! teclado em modo `RIDEV_INPUTSINK` (recebe as teclas mesmo com outra app em foco). Ao
//! contrário de um hook `WH_KEYBOARD_LL`, o raw input nunca está no caminho do teclado:
//! se esta thread demorar, o utilizador não nota. A thread de UI só acorda para as três teclas.
//!
//! O estado vem da própria tecla: lê-se uma vez no arranque e inverte-se em cada premir
//! (sem repetições: com a tecla premida o Windows repete o "make"). Um `GetKeyState` aqui
//! não serve: esta thread não recebe teclado, por isso o estado dela fica desatualizado.

use crate::EventProxy;
use island_core::Event;
use island_core::event::{LockKey, SystemEvent};
use std::cell::{Cell, RefCell};
use std::io;
use std::thread;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RID_INPUT,
    RegisterRawInputDevices, RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL, VK_NUMLOCK, VK_SCROLL};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, RegisterClassExW,
    TranslateMessage, WM_INPUT, WNDCLASSEXW, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::w;

/// `RAWKEYBOARD::Flags`: bit 0 = a tecla foi largada.
const RI_KEY_BREAK: u16 = 1;

/// Estado das três teclas e se estão premidas (para ignorar repetições).
#[derive(Debug, Default)]
struct Locks {
    on: [bool; 3],
    down: [bool; 3],
}

fn index(vk: u16) -> Option<(usize, LockKey)> {
    match vk {
        v if v == VK_CAPITAL.0 => Some((0, LockKey::Caps)),
        v if v == VK_NUMLOCK.0 => Some((1, LockKey::Num)),
        v if v == VK_SCROLL.0 => Some((2, LockKey::Scroll)),
        _ => None,
    }
}

impl Locks {
    /// Uma tecla premida (`released: false`) ou largada. Devolve o aviso, se a tecla mudou.
    fn key(&mut self, vk: u16, released: bool) -> Option<SystemEvent> {
        let (i, key) = index(vk)?;
        if released {
            self.down[i] = false;
            return None;
        }
        if std::mem::replace(&mut self.down[i], true) {
            return None; // repetição automática
        }
        self.on[i] = !self.on[i];
        Some(SystemEvent::Lock { key, on: self.on[i] })
    }
}

thread_local! {
    static LOCKS: RefCell<Locks> = RefCell::new(Locks::default());
    static PROXY: RefCell<Option<EventProxy>> = const { RefCell::new(None) };
    static READY: Cell<bool> = const { Cell::new(false) };
}

pub fn spawn(proxy: EventProxy) -> io::Result<()> {
    thread::Builder::new().name("keys".into()).spawn(move || {
        if let Err(e) = run(proxy) {
            log!("teclas de bloqueio: {e}");
        }
    })?;
    Ok(())
}

fn run(proxy: EventProxy) -> windows::core::Result<()> {
    // SAFETY: janela e classe criadas e usadas só nesta thread; o `wndproc` tem a assinatura certa.
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: w!("DynamicIslandKeys"),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("DynamicIslandKeys"),
            w!("Dynamic Island teclas"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )?;
        LOCKS.with_borrow_mut(|l| {
            for (i, vk) in [VK_CAPITAL, VK_NUMLOCK, VK_SCROLL].into_iter().enumerate() {
                // Só aqui, uma vez: o bit de baixo é o estado "ligado".
                l.on[i] = GetKeyState(i32::from(vk.0)) & 1 != 0;
            }
        });
        PROXY.set(Some(proxy));
        let device = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 6, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
        RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32)?;
        READY.set(true);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_INPUT
        && READY.get()
        && let Some((vk, released)) = read_key(lparam)
        && let Some(event) = LOCKS.with_borrow_mut(|l| l.key(vk, released))
    {
        PROXY.with_borrow(|p| {
            if let Some(p) = p {
                p.send(Event::System(event));
            }
        });
    }
    // SAFETY: argumentos recebidos do sistema, passados tal e qual (WM_INPUT pede a chamada
    // ao `DefWindowProc` para o sistema limpar o evento).
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A tecla (código virtual) e se foi largada, de um `WM_INPUT` de teclado.
fn read_key(lparam: LPARAM) -> Option<(u16, bool)> {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    // SAFETY: `raw` é local e `size` diz o seu tamanho; o sistema escreve no máximo isso.
    let copied = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as _),
            RID_INPUT,
            Some(&mut raw as *mut RAWINPUT as *mut _),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if copied == u32::MAX || raw.header.dwType != RIM_TYPEKEYBOARD.0 {
        return None;
    }
    // SAFETY: `dwType` confirma que a union guarda um `RAWKEYBOARD`.
    let key = unsafe { raw.data.keyboard };
    Some((key.VKey, key.Flags & RI_KEY_BREAK != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_on_each_press_and_ignores_auto_repeat() {
        let mut l = Locks::default();
        let caps = VK_CAPITAL.0;
        assert_eq!(l.key(caps, false), Some(SystemEvent::Lock { key: LockKey::Caps, on: true }));
        // Com a tecla premida o Windows repete o "make": não conta.
        assert_eq!(l.key(caps, false), None);
        assert_eq!(l.key(caps, false), None);
        assert_eq!(l.key(caps, true), None);
        assert_eq!(l.key(caps, false), Some(SystemEvent::Lock { key: LockKey::Caps, on: false }));
    }

    #[test]
    fn keys_are_independent_and_others_are_ignored() {
        let mut l = Locks::default();
        l.on[1] = true; // o Num Lock já estava ligado ao arrancar
        assert_eq!(l.key(VK_NUMLOCK.0, false), Some(SystemEvent::Lock { key: LockKey::Num, on: false }));
        assert_eq!(l.key(VK_SCROLL.0, false), Some(SystemEvent::Lock { key: LockKey::Scroll, on: true }));
        assert_eq!(l.key(0x41, false), None); // 'A'
    }
}
