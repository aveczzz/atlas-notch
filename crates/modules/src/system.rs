//! Avisos curtos do sistema: Caps/Num/Scroll Lock, carregador e bateria, dispositivo de
//! áudio e "Copiado". Um aviso novo substitui o anterior (nunca empilha).

use island_core::event::{CopyKind, Event, LockKey, SystemEvent};
use island_core::view::{Compact, Icon, Priority, View};
use island_core::{Action, Command, Module, ModuleId};
use std::time::{Duration, Instant};

pub const ID: ModuleId = "system";

/// Teclas e "Copiado" acontecem a toda a hora: aparecem pouco tempo.
const SHORT: Duration = Duration::from_millis(1500);
/// Bateria e dispositivos: dá-se tempo para ler o nome.
const LONG: Duration = Duration::from_millis(2500);
/// Nomes de dispositivos compridos cortam-se para o aviso não crescer sem fim.
const MAX_NAME: usize = 28;

#[derive(Debug, Default)]
pub struct SystemModule {
    shown: Option<(SystemEvent, Instant)>,
}

fn lock_name(key: LockKey) -> &'static str {
    match key {
        LockKey::Caps => "Caps Lock",
        LockKey::Num => "Num Lock",
        LockKey::Scroll => "Scroll Lock",
    }
}

fn short(name: &str) -> String {
    if name.chars().count() <= MAX_NAME {
        return name.to_owned();
    }
    let cut: String = name.chars().take(MAX_NAME - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Ícone e texto do aviso.
fn describe(e: &SystemEvent) -> (Icon, String) {
    match e {
        SystemEvent::Lock { key, on } => {
            (Icon::Keyboard, format!("{} {}", lock_name(*key), if *on { "ativado" } else { "desativado" }))
        }
        SystemEvent::Power { charging: true, percent } => (Icon::BatteryCharging, format!("A carregar · {percent}%")),
        SystemEvent::Power { charging: false, percent } => (Icon::Battery, format!("Sem carregador · {percent}%")),
        SystemEvent::BatteryLow { percent } => (Icon::Battery, format!("Bateria fraca · {percent}%")),
        SystemEvent::AudioDevice { name } => (Icon::Headphones, short(name)),
        SystemEvent::Copied(kind) => (
            Icon::Copy,
            match kind {
                CopyKind::Text => "Copiado · texto".to_owned(),
                CopyKind::Image => "Copiado · imagem".to_owned(),
                CopyKind::Files(1) => "Copiado · 1 ficheiro".to_owned(),
                CopyKind::Files(n) => format!("Copiado · {n} ficheiros"),
            },
        ),
    }
}

fn duration(e: &SystemEvent) -> Duration {
    match e {
        SystemEvent::Lock { .. } | SystemEvent::Copied(_) => SHORT,
        _ => LONG,
    }
}

impl Module for SystemModule {
    fn id(&self) -> ModuleId {
        ID
    }

    fn on_event(&mut self, event: &Event, now: Instant) -> bool {
        let Event::System(e) = event else { return false };
        self.shown = Some((e.clone(), now + duration(e)));
        true
    }

    fn on_action(&mut self, _action: &Action, _now: Instant, _out: &mut Vec<Command>) -> bool {
        false
    }

    fn view(&self, now: Instant) -> Option<View> {
        let (e, until) = self.shown.as_ref()?;
        if now >= *until {
            return None;
        }
        let (icon, text) = describe(e);
        // `Attention`: é o resultado direto de algo que o utilizador acabou de fazer.
        Some(View {
            priority: Priority::Attention,
            compact: Compact { icon, text, progress: None, pulse: false, toast: None },
            expanded: Vec::new(),
        })
    }

    fn deadline(&self) -> Option<Instant> {
        self.shown.as_ref().map(|(_, until)| *until)
    }

    fn on_deadline(&mut self, now: Instant) {
        if self.shown.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.shown = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(m: &mut SystemModule, e: SystemEvent, now: Instant) -> bool {
        m.on_event(&Event::System(e), now)
    }

    #[test]
    fn caps_lock_shows_state_and_expires_fast() {
        let t0 = Instant::now();
        let mut m = SystemModule::default();
        assert!(m.view(t0).is_none());
        assert!(send(&mut m, SystemEvent::Lock { key: LockKey::Caps, on: true }, t0));
        let v = m.view(t0).unwrap();
        assert_eq!(v.compact.icon, Icon::Keyboard);
        assert_eq!(v.compact.text, "Caps Lock ativado");
        assert_eq!(m.deadline(), Some(t0 + SHORT));
        m.on_deadline(t0 + SHORT);
        assert!(m.view(t0 + SHORT).is_none());
        assert_eq!(m.deadline(), None);
    }

    #[test]
    fn a_new_notice_replaces_the_old_one() {
        let t0 = Instant::now();
        let mut m = SystemModule::default();
        send(&mut m, SystemEvent::Lock { key: LockKey::Num, on: false }, t0);
        send(&mut m, SystemEvent::Power { charging: true, percent: 54 }, t0 + Duration::from_millis(300));
        let v = m.view(t0 + Duration::from_millis(300)).unwrap();
        assert_eq!(v.compact.icon, Icon::BatteryCharging);
        assert_eq!(v.compact.text, "A carregar · 54%");
        // O prazo é o do aviso novo, contado desde que chegou.
        assert_eq!(m.deadline(), Some(t0 + Duration::from_millis(300) + LONG));
    }

    #[test]
    fn power_and_low_battery_texts() {
        assert_eq!(describe(&SystemEvent::Power { charging: false, percent: 80 }).1, "Sem carregador · 80%");
        assert_eq!(describe(&SystemEvent::BatteryLow { percent: 20 }).1, "Bateria fraca · 20%");
    }

    #[test]
    fn long_device_names_are_cut() {
        let name = "Altifalantes (Realtek High Definition Audio com um nome muito comprido)";
        let (icon, text) = describe(&SystemEvent::AudioDevice { name: name.into() });
        assert_eq!(icon, Icon::Headphones);
        assert!(text.chars().count() <= MAX_NAME);
        assert!(text.ends_with('…'));
        assert_eq!(describe(&SystemEvent::AudioDevice { name: "AirPods".into() }).1, "AirPods");
    }

    #[test]
    fn copied_says_only_the_kind() {
        assert_eq!(describe(&SystemEvent::Copied(CopyKind::Text)).1, "Copiado · texto");
        assert_eq!(describe(&SystemEvent::Copied(CopyKind::Image)).1, "Copiado · imagem");
        assert_eq!(describe(&SystemEvent::Copied(CopyKind::Files(1))).1, "Copiado · 1 ficheiro");
        assert_eq!(describe(&SystemEvent::Copied(CopyKind::Files(3))).1, "Copiado · 3 ficheiros");
    }

    #[test]
    fn ignores_other_events() {
        let mut m = SystemModule::default();
        assert!(!m.on_event(&Event::Media(island_core::event::MediaEvent::Cleared), Instant::now()));
    }
}
