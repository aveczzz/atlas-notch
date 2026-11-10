//! Volume geral (HUD quando muda) e misturador por app.
//!
//! Em repouso tem prioridade `Ambient`: se não houver mais nada, clicar na island
//! abre o misturador.
//!
//! Os ecos dos nossos próprios comandos chegam marcados (`quiet`) pela fonte, que os
//! reconhece pelo `event_context` do Core Audio: não é preciso adivinhar pelo valor.

use island_core::command::Command;
use island_core::event::{AudioEvent, AudioSession, Event};
use island_core::view::{Compact, ControlId, Icon, MixerWidget, Priority, Row, View, VolumeLine, Widget};
use island_core::{Action, ActionKind, Module, ModuleId};
use std::time::{Duration, Instant};

pub const ID: ModuleId = "mixer";
const MASTER: ControlId = ControlId::new("master");
const SESSION: &str = "session";
const MUTE: &str = "mute";
const HUD_DURATION: Duration = Duration::from_millis(1500);

#[derive(Debug, Default)]
pub struct MixerModule {
    master: f32,
    master_muted: bool,
    /// Cada sessão tem uma chave estável, usada como `ControlId::index`.
    /// Assim um clique nunca vai parar à sessão errada se a lista mudar entretanto.
    sessions: Vec<(u32, AudioSession)>,
    next_key: u32,
    hud_until: Option<Instant>,
}

impl MixerModule {
    fn key_for(&mut self, id: &str) -> u32 {
        if let Some((k, _)) = self.sessions.iter().find(|(_, s)| s.id == id) {
            return *k;
        }
        self.next_key += 1;
        self.next_key
    }
}

impl Module for MixerModule {
    fn id(&self) -> ModuleId {
        ID
    }

    fn on_event(&mut self, event: &Event, now: Instant) -> bool {
        let Event::Audio(e) = event else { return false };
        match e {
            AudioEvent::MasterVolume { volume, muted, quiet } => {
                if (*volume, *muted) == (self.master, self.master_muted) {
                    return false;
                }
                self.master = *volume;
                self.master_muted = *muted;
                // Eco de um comando nosso, estado inicial ou troca de dispositivo: sem HUD.
                if !quiet {
                    self.hud_until = Some(now + HUD_DURATION);
                }
                true
            }
            AudioEvent::Sessions(list) => {
                let next: Vec<_> = list.iter().map(|s| (self.key_for(&s.id), s.clone())).collect();
                let changed = next != self.sessions;
                self.sessions = next;
                changed
            }
            AudioEvent::SessionVolume { id, volume, muted } => {
                let Some((_, s)) = self.sessions.iter_mut().find(|(_, s)| &s.id == id) else {
                    return false;
                };
                let changed = (s.volume, s.muted) != (*volume, *muted);
                s.volume = *volume;
                s.muted = *muted;
                changed
            }
        }
    }

    fn on_action(&mut self, action: &Action, _now: Instant, out: &mut Vec<Command>) -> bool {
        if action.kind == ActionKind::Press && action.control.name == MUTE {
            if let Some((_, s)) = self.sessions.iter().find(|(k, _)| *k == action.control.index) {
                out.push(Command::SetSessionMute { id: s.id.clone(), muted: !s.muted });
            }
            return false;
        }
        let ActionKind::SetValue(v) = action.kind else { return false };
        let v = v.clamp(0.0, 1.0);
        if action.control == MASTER {
            out.push(Command::SetMasterVolume(v));
        } else if action.control.name == SESSION {
            let Some((_, s)) = self.sessions.iter().find(|(k, _)| *k == action.control.index) else {
                return false;
            };
            out.push(Command::SetSessionVolume { id: s.id.clone(), volume: v });
        }
        false // o estado real chega pelo evento de confirmação
    }

    fn view(&self, now: Instant) -> Option<View> {
        let hud = self.hud_until.is_some_and(|t| now < t);
        let icon = if self.master_muted { Icon::Muted } else { Icon::Volume };
        let mut expanded = vec![Row::Slider { control: MASTER, label: "Geral".into(), value: self.master }];
        expanded.extend(self.sessions.iter().map(|(k, s)| Row::Slider {
            control: ControlId::indexed(SESSION, *k),
            label: s.name.clone(),
            value: if s.muted { 0.0 } else { s.volume },
        }));
        Some(View {
            priority: if hud { Priority::Attention } else { Priority::Ambient },
            compact: Compact {
                icon,
                text: String::new(),
                progress: hud.then_some(if self.master_muted { 0.0 } else { self.master }),
                pulse: false,
                toast: None,
            },
            expanded,
        })
    }

    /// O separador "Som": volume geral e uma linha por app, com chave estável no controlo.
    fn widget(&self, _now: Instant) -> Option<Widget> {
        let mut lines = vec![VolumeLine {
            label: "Volume geral".into(),
            value: self.master,
            muted: self.master_muted,
            slider: MASTER,
            mute: None,
        }];
        lines.extend(self.sessions.iter().map(|(k, s)| VolumeLine {
            label: s.name.clone(),
            value: s.volume,
            muted: s.muted,
            slider: ControlId::indexed(SESSION, *k),
            mute: Some(ControlId::indexed(MUTE, *k)),
        }));
        Some(Widget::Mixer(MixerWidget { lines }))
    }

    fn deadline(&self) -> Option<Instant> {
        self.hud_until
    }

    fn on_deadline(&mut self, now: Instant) {
        if self.hud_until.is_some_and(|t| t <= now) {
            self.hud_until = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, vol: f32) -> AudioSession {
        AudioSession { id: id.into(), name: id.into(), volume: vol, muted: false }
    }

    fn master(volume: f32, quiet: bool) -> Event {
        Event::Audio(AudioEvent::MasterVolume { volume, muted: false, quiet })
    }

    #[test]
    fn volume_change_shows_hud_then_expires() {
        let t0 = Instant::now();
        let mut m = MixerModule::default();
        assert_eq!(m.view(t0).unwrap().priority, Priority::Ambient);
        m.on_event(&master(0.5, false), t0);
        assert_eq!(m.view(t0).unwrap().priority, Priority::Attention);
        let d = m.deadline().unwrap();
        m.on_deadline(d);
        assert_eq!(m.view(d).unwrap().priority, Priority::Ambient);
        assert!(m.deadline().is_none());
    }

    #[test]
    fn startup_snapshot_and_own_changes_do_not_pop_hud() {
        let t0 = Instant::now();
        let mut m = MixerModule::default();
        assert!(m.on_event(&master(0.7, true), t0), "o valor atualiza");
        assert!(m.hud_until.is_none(), "o arranque não abre o HUD");

        let mut out = vec![];
        m.on_action(&Action { module: ID, control: MASTER, kind: ActionKind::SetValue(0.3) }, t0, &mut out);
        assert_eq!(out, vec![Command::SetMasterVolume(0.3)]);
        // Ao arrastar o slider chegam vários ecos intermédios: nenhum abre o HUD.
        for v in [0.6, 0.45, 0.3] {
            m.on_event(&master(v, true), t0);
        }
        assert!(m.hud_until.is_none());
        assert_eq!(m.master, 0.3);
        // Uma mudança de fora (teclas de volume) abre.
        m.on_event(&master(0.32, false), t0);
        assert!(m.hud_until.is_some());
    }

    #[test]
    fn widget_lists_master_then_apps_and_mute_toggles() {
        let t0 = Instant::now();
        let mut m = MixerModule::default();
        m.on_event(&master(0.4, true), t0);
        m.on_event(&Event::Audio(AudioEvent::Sessions(vec![session("Spotify", 0.8)])), t0);
        let Some(Widget::Mixer(w)) = m.widget(t0) else { panic!() };
        let labels: Vec<_> = w.lines.iter().map(|l| (l.label.as_str(), l.value)).collect();
        assert_eq!(labels, vec![("Volume geral", 0.4), ("Spotify", 0.8)]);
        assert!(w.lines[0].mute.is_none(), "o geral não tem botão de mute");

        let mut out = vec![];
        let mute = w.lines[1].mute.unwrap();
        m.on_action(&Action { module: ID, control: mute, kind: ActionKind::Press }, t0, &mut out);
        assert_eq!(out, vec![Command::SetSessionMute { id: "Spotify".into(), muted: true }]);
    }

    #[test]
    fn session_actions_use_stable_keys() {
        let t0 = Instant::now();
        let mut m = MixerModule::default();
        m.on_event(&Event::Audio(AudioEvent::Sessions(vec![session("spotify", 1.0), session("teams", 1.0)])), t0);
        let teams_key = m.sessions[1].0;
        // A lista muda (spotify fecha) mas a chave do teams mantém-se.
        m.on_event(&Event::Audio(AudioEvent::Sessions(vec![session("teams", 1.0)])), t0);
        let mut out = vec![];
        let action = Action {
            module: ID,
            control: ControlId::indexed(SESSION, teams_key),
            kind: ActionKind::SetValue(0.2),
        };
        m.on_action(&action, t0, &mut out);
        assert_eq!(out, vec![Command::SetSessionVolume { id: "teams".into(), volume: 0.2 }]);
    }
}
