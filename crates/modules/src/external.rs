//! Conteúdo enviado por apps externas através do named pipe (mensagens `show`/`clear`).
//!
//! É a forma de "adicionar uma app" sem tocar no código da island: um script de
//! backup, um deploy, um bot, qualquer coisa que consiga escrever uma linha de JSON.

use island_core::command::Command;
use island_core::event::Event;
use island_core::view::{Compact, Icon, Priority, Row, View, Widget};
use island_core::{Action, Module, ModuleId};
use island_proto::{Message, Show};
use std::time::{Duration, Instant};

pub const ID: ModuleId = "external";
/// Número máximo de fontes simultâneas. Uma app com bugs não pode encher a memória.
const MAX_SOURCES: usize = 16;

#[derive(Debug)]
struct Entry {
    show: Show,
    priority: Priority,
    expires: Option<Instant>,
    updated: Instant,
}

#[derive(Debug, Default)]
pub struct ExternalModule {
    entries: Vec<Entry>,
}

impl ExternalModule {
    fn top(&self) -> Option<&Entry> {
        self.entries.iter().max_by_key(|e| (e.priority, e.updated))
    }
}

impl Module for ExternalModule {
    fn id(&self) -> ModuleId {
        ID
    }

    fn on_event(&mut self, event: &Event, now: Instant) -> bool {
        match event {
            Event::External(Message::Show(show)) => {
                let entry = Entry {
                    show: show.clone(),
                    priority: show.priority.into(),
                    expires: show.ttl_ms.map(|ms| now + Duration::from_millis(ms)),
                    updated: now,
                };
                if let Some(e) = self.entries.iter_mut().find(|e| e.show.source == show.source) {
                    *e = entry;
                } else {
                    if self.entries.len() >= MAX_SOURCES {
                        // Remove a entrada menos recente para dar lugar.
                        if let Some(i) = (0..self.entries.len()).min_by_key(|&i| self.entries[i].updated) {
                            self.entries.swap_remove(i);
                        }
                    }
                    self.entries.push(entry);
                }
                true
            }
            Event::External(Message::Clear { source }) => {
                let before = self.entries.len();
                self.entries.retain(|e| &e.show.source != source);
                self.entries.len() != before
            }
            _ => false,
        }
    }

    fn on_action(&mut self, _action: &Action, _now: Instant, _out: &mut Vec<Command>) -> bool {
        // Ações de volta para apps externas: ver "Ideias futuras" no CLAUDE.md.
        false
    }

    fn view(&self, _now: Instant) -> Option<View> {
        let top = self.top()?;
        let mut expanded = Vec::with_capacity(self.entries.len() * 2);
        for e in &self.entries {
            expanded.push(Row::Text { primary: e.show.title.clone(), secondary: e.show.detail.clone() });
            if let Some(p) = e.show.progress {
                expanded.push(Row::Progress(p));
            }
        }
        Some(View {
            priority: top.priority,
            compact: Compact {
                icon: Icon::Named(top.show.source.clone()),
                text: top.show.title.clone(),
                progress: top.show.progress,
                pulse: false,
                toast: None,
            },
            expanded,
        })
    }

    fn widget(&self, now: Instant) -> Option<Widget> {
        Some(Widget::Rows(self.view(now)?.expanded))
    }

    fn deadline(&self) -> Option<Instant> {
        self.entries.iter().filter_map(|e| e.expires).min()
    }

    fn on_deadline(&mut self, now: Instant) {
        self.entries.retain(|e| e.expires.is_none_or(|t| t > now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_proto::WirePriority;

    fn show(source: &str, ttl: Option<u64>) -> Event {
        Event::External(Message::Show(Show {
            source: source.into(),
            title: format!("{source} title"),
            detail: None,
            priority: WirePriority::Info,
            ttl_ms: ttl,
            progress: Some(0.5),
        }))
    }

    #[test]
    fn show_update_clear() {
        let t0 = Instant::now();
        let mut m = ExternalModule::default();
        m.on_event(&show("backup", None), t0);
        m.on_event(&show("backup", None), t0);
        assert_eq!(m.entries.len(), 1, "mesma fonte substitui");
        assert_eq!(m.view(t0).unwrap().compact.progress, Some(0.5));
        assert!(m.on_event(&Event::External(Message::Clear { source: "backup".into() }), t0));
        assert!(m.view(t0).is_none());
    }

    #[test]
    fn ttl_expires() {
        let t0 = Instant::now();
        let mut m = ExternalModule::default();
        m.on_event(&show("deploy", Some(1000)), t0);
        let d = m.deadline().unwrap();
        m.on_deadline(d);
        assert!(m.view(d).is_none());
    }

    #[test]
    fn bounded_sources() {
        let t0 = Instant::now();
        let mut m = ExternalModule::default();
        for i in 0..50u64 {
            m.on_event(&show(&format!("s{i}"), None), t0 + Duration::from_millis(i));
        }
        assert_eq!(m.entries.len(), MAX_SOURCES);
        assert!(m.entries.iter().any(|e| e.show.source == "s49"), "a mais recente fica");
    }
}
