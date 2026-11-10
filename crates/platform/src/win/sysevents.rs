//! Avisos do sistema que chegam como mensagens da janela da island: ligar e desligar o
//! carregador, bateria fraca e "Copiado". Nada de polling: o Windows avisa.
//!
//! - Energia: `RegisterPowerSettingNotification` (fonte de energia e percentagem da bateria)
//!   → `WM_POWERBROADCAST`.
//! - Clipboard: `AddClipboardFormatListener` → `WM_CLIPBOARDUPDATE`. **Só se olha para o tipo**
//!   (texto, imagem, ficheiros); o conteúdo nunca é lido. Apps que marcam o conteúdo como
//!   sensível (gestores de palavras-passe) são ignoradas.

use super::window::{Input, push};
use island_core::event::{CopyKind, SystemEvent};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, RemoveClipboardFormatListener,
};
use windows::Win32::System::Power::{
    GetSystemPowerStatus, HPOWERNOTIFY, POWERBROADCAST_SETTING, RegisterPowerSettingNotification,
    SYSTEM_POWER_STATUS, UnregisterPowerSettingNotification,
};
use windows::Win32::System::SystemServices::{GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_WINDOW_HANDLE, PBT_POWERSETTINGCHANGE};
use windows::core::w;

/// Mensagem do clipboard (`WM_CLIPBOARDUPDATE`); está em `DataExchange` mas é só um número.
pub const WM_CLIPBOARDUPDATE: u32 = 0x031D;
/// `WM_POWERBROADCAST`.
pub const WM_POWERBROADCAST: u32 = 0x0218;

/// O Excel e outras apps atualizam o clipboard várias vezes seguidas: um aviso por copiar.
const COPY_DEBOUNCE: Duration = Duration::from_millis(300);
/// Formatos do clipboard.
const CF_TEXT: u32 = 1;
const CF_BITMAP: u32 = 2;
const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
/// Limiares de bateria fraca, do mais alto para o mais baixo.
const LOW: [u8; 2] = [20, 10];

/// Estado da energia. Puro: testa-se sem Windows.
#[derive(Debug)]
struct PowerTracker {
    /// Ligado à corrente. `None` até à primeira notificação (o Windows manda o estado
    /// atual ao registar: não é uma mudança).
    ac: Option<bool>,
    /// Último limiar de bateria fraca já avisado (100 = nenhum).
    warned: u8,
}

impl PowerTracker {
    fn new() -> Self {
        Self { ac: None, warned: 100 }
    }

    /// A fonte de energia mudou (ou chegou o estado inicial).
    fn on_ac(&mut self, ac: bool, percent: u8) -> Option<SystemEvent> {
        let changed = self.ac.is_some_and(|before| before != ac);
        self.ac = Some(ac);
        if ac {
            self.warned = 100;
        }
        changed.then_some(SystemEvent::Power { charging: ac, percent })
    }

    /// A percentagem mudou. Avisa uma vez ao passar cada limiar, e só sem carregador.
    fn on_percent(&mut self, percent: u8) -> Option<SystemEvent> {
        if self.ac != Some(false) {
            return None;
        }
        if percent > LOW[0] {
            self.warned = 100; // recuperou (ou estava carregada): volta a avisar mais tarde
            return None;
        }
        // O limiar mais baixo já atingido: uma descida grande (40% -> 8%) avisa uma vez só.
        let threshold = LOW.into_iter().rev().find(|&t| percent <= t)?;
        if self.warned <= threshold {
            return None;
        }
        self.warned = threshold;
        Some(SystemEvent::BatteryLow { percent })
    }
}

thread_local! {
    static POWER: RefCell<PowerTracker> = RefCell::new(PowerTracker::new());
    static LAST_COPY: Cell<Option<Instant>> = const { Cell::new(None) };
    static SENSITIVE_FORMAT: Cell<u32> = const { Cell::new(0) };
}

/// Os registos que têm de ser desfeitos no `WM_DESTROY`.
#[derive(Debug)]
pub struct Registrations {
    power: Vec<HPOWERNOTIFY>,
}

/// Regista energia e clipboard. Falhas só desligam o aviso respetivo.
pub fn register(hwnd: HWND) -> Registrations {
    let mut power = Vec::new();
    for guid in [GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING] {
        // SAFETY: `hwnd` é a nossa janela; o GUID vive durante a chamada.
        match unsafe { RegisterPowerSettingNotification(HANDLE(hwnd.0), &guid, DEVICE_NOTIFY_WINDOW_HANDLE) } {
            Ok(h) => power.push(h),
            Err(e) => log!("energia: {e}"),
        }
    }
    // SAFETY: `hwnd` é a nossa janela; o nome é um literal.
    unsafe {
        if let Err(e) = AddClipboardFormatListener(hwnd) {
            log!("clipboard: {e}");
        }
        SENSITIVE_FORMAT.set(RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")));
    }
    Registrations { power }
}

pub fn unregister(hwnd: HWND, regs: Registrations) {
    // SAFETY: os mesmos registos feitos em `register`, na mesma janela.
    unsafe {
        let _ = RemoveClipboardFormatListener(hwnd);
        for h in regs.power {
            let _ = UnregisterPowerSettingNotification(h);
        }
    }
}

/// Percentagem atual da bateria, ou `None` sem bateria (desktop) ou se o Windows não sabe.
fn battery_percent() -> Option<u8> {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` é local.
    unsafe { GetSystemPowerStatus(&mut status).ok()? };
    // BatteryFlag 128 = sem bateria; BatteryLifePercent 255 = desconhecido.
    (status.BatteryFlag & 128 == 0 && status.BatteryLifePercent <= 100).then_some(status.BatteryLifePercent)
}

/// `WM_POWERBROADCAST`.
pub fn on_power_broadcast(wparam: WPARAM, lparam: LPARAM) {
    if wparam.0 as u32 != PBT_POWERSETTINGCHANGE || lparam.0 == 0 {
        return;
    }
    // SAFETY: nesta notificação o `lparam` aponta para um `POWERBROADCAST_SETTING` válido
    // durante a mensagem; só se lê o DWORD de dados quando `DataLength` o garante.
    let (guid, value) = unsafe {
        let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
        if (setting.DataLength as usize) < size_of::<u32>() {
            return;
        }
        (setting.PowerSetting, std::ptr::read_unaligned(setting.Data.as_ptr().cast::<u32>()))
    };
    let Some(percent_now) = battery_percent() else { return }; // desktop: sem avisos
    let event = POWER.with_borrow_mut(|p| {
        if guid == GUID_ACDC_POWER_SOURCE {
            // 0 = corrente, 1 = bateria, 2 = UPS.
            p.on_ac(value == 0, percent_now)
        } else if guid == GUID_BATTERY_PERCENTAGE_REMAINING {
            p.on_percent(value.min(100) as u8)
        } else {
            None
        }
    });
    if let Some(e) = event {
        push(Input::System(e));
    }
}

/// `WM_CLIPBOARDUPDATE`.
pub fn on_clipboard_update() {
    let now = Instant::now();
    if LAST_COPY.get().is_some_and(|t| now.duration_since(t) < COPY_DEBOUNCE) {
        return;
    }
    // SAFETY: consultas de formato, sem abrir o clipboard.
    let available = |format: u32| unsafe { IsClipboardFormatAvailable(format).is_ok() };
    let sensitive = SENSITIVE_FORMAT.get();
    if sensitive != 0 && available(sensitive) {
        return;
    }
    let kind = if available(CF_HDROP) {
        CopyKind::Files(file_count())
    } else if available(CF_DIB) || available(CF_BITMAP) {
        CopyKind::Image
    } else if available(CF_UNICODETEXT) || available(CF_TEXT) {
        CopyKind::Text
    } else {
        return; // limpou-se o clipboard, ou é um formato que não interessa
    };
    LAST_COPY.set(Some(now));
    push(Input::System(SystemEvent::Copied(kind)));
}

/// Quantos ficheiros estão no clipboard (0 se não for possível contar).
fn file_count() -> u32 {
    // SAFETY: o clipboard abre-se e fecha-se aqui; só se pede o número de ficheiros, e o
    // handle é do clipboard (não se liberta). Se outra app o tiver aberto, devolve 0.
    unsafe {
        if OpenClipboard(None).is_err() {
            return 0;
        }
        let count = GetClipboardData(CF_HDROP).map_or(0, |h| DragQueryFileW(HDROP(h.0), u32::MAX, None));
        let _ = CloseClipboard();
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_power_report_is_not_a_change() {
        let mut p = PowerTracker::new();
        assert_eq!(p.on_ac(true, 80), None);
        assert_eq!(p.on_ac(true, 80), None);
        assert_eq!(p.on_ac(false, 80), Some(SystemEvent::Power { charging: false, percent: 80 }));
        assert_eq!(p.on_ac(true, 79), Some(SystemEvent::Power { charging: true, percent: 79 }));
    }

    #[test]
    fn low_battery_warns_once_per_threshold_and_only_unplugged() {
        let mut p = PowerTracker::new();
        p.on_ac(false, 50);
        assert_eq!(p.on_percent(25), None);
        assert_eq!(p.on_percent(20), Some(SystemEvent::BatteryLow { percent: 20 }));
        assert_eq!(p.on_percent(19), None, "o aviso dos 20% já saiu");
        assert_eq!(p.on_percent(10), Some(SystemEvent::BatteryLow { percent: 10 }));
        assert_eq!(p.on_percent(9), None);
        // Ligar o carregador repõe os avisos.
        p.on_ac(true, 9);
        assert_eq!(p.on_percent(9), None, "com carregador não avisa");
        p.on_ac(false, 9);
        assert_eq!(p.on_percent(9), Some(SystemEvent::BatteryLow { percent: 9 }));
    }

    #[test]
    fn a_big_drop_skips_straight_to_the_lowest_threshold() {
        let mut p = PowerTracker::new();
        p.on_ac(false, 40);
        assert_eq!(p.on_percent(8), Some(SystemEvent::BatteryLow { percent: 8 }));
        assert_eq!(p.on_percent(7), None);
    }
}
