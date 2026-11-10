//! Notificações do Windows (Outlook, Teams, ...), lidas pelo `UserNotificationListener`.

use island_core::command::Command;
use island_core::event::{Event, NotificationEvent};
use island_core::view::{Button, Compact, ControlId, Icon, Priority, Row, Toast, View};
use island_core::{Action, ActionKind, Module, ModuleId};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const ID: ModuleId = "notifications";
const DISMISS: &str = "dismiss";
const OPEN: &str = "open";
const TOAST_VISIBLE: Duration = Duration::from_secs(5);
const HISTORY: usize = 20;
const SHOWN_WHEN_EXPANDED: usize = 5;

#[derive(Debug, Default)]
pub struct NotificationsModule {
    /// Mais recente à frente.
    recent: VecDeque<NotificationEvent>,
    toast_until: Option<Instant>,
}

fn icon_for(app: &str) -> Icon {
    let app = app.to_ascii_lowercase();
    if app.contains("outlook") || app.contains("mail") {
        Icon::Mail
    } else {
        Icon::Bell
    }
}

impl Module for NotificationsModule {
    fn id(&self) -> ModuleId {
        ID
    }

    fn on_event(&mut self, event: &Event, now: Instant) -> bool {
        let Event::Notification(n) = event else { return false };
        // O listener pode reenviar a mesma notificação; deduplicar por id.
        if self.recent.iter().any(|r| r.id == n.id) {
            return false;
        }
        self.recent.push_front(n.clone());
        self.recent.truncate(HISTORY);
        self.toast_until = Some(now + TOAST_VISIBLE);
        true
    }

    fn on_action(&mut self, action: &Action, _now: Instant, out: &mut Vec<Command>) -> bool {
        if action.kind != ActionKind::Press {
            return false;
        }
        if action.control.name == OPEN {
            // Clicar no toast: abre a app da notificação e fecha o toast.
            let id = action.control.index;
            if let Some(aumid) = self.recent.iter().find(|n| n.id == id).and_then(|n| n.aumid.clone()) {
                out.push(Command::OpenApp { aumid });
            }
            self.toast_until = None;
            return true;
        }
        if action.control.name != DISMISS {
            return false;
        }
        let id = action.control.index;
        let before = self.recent.len();
        self.recent.retain(|n| n.id != id);
        if self.recent.len() == before {
            return false;
        }
        out.push(Command::DismissNotification { id });
        true
    }

    fn view(&self, now: Instant) -> Option<View> {
        let latest = self.recent.front()?;
        let toast = self.toast_until.is_some_and(|t| now < t);
        if !toast {
            // Fora do toast, este módulo não pede espaço na island.
            return None;
        }
        let expanded = self
            .recent
            .iter()
            .take(SHOWN_WHEN_EXPANDED)
            .flat_map(|n| {
                [
                    Row::Text { primary: format!("{} · {}", n.app, n.title), secondary: n.body.clone() },
                    Row::Buttons(vec![Button {
                        control: ControlId::indexed(DISMISS, n.id),
                        icon: None,
                        label: Some("Dispensar".into()),
                    }]),
                ]
            })
            .collect();
        Some(View {
            priority: Priority::Info,
            compact: Compact {
                icon: icon_for(&latest.app),
                text: latest.title.clone(),
                progress: None,
                pulse: false,
                toast: Some(Toast {
                    app: latest.app.clone(),
                    body: latest.body.clone().unwrap_or_default(),
                    icon: latest.icon.clone(),
                    open: ControlId::indexed(OPEN, latest.id),
                }),
            },
            expanded,
        })
    }

    fn deadline(&self) -> Option<Instant> {
        self.toast_until
    }

    fn on_deadline(&mut self, now: Instant) {
        if self.toast_until.is_some_and(|t| t <= now) {
            self.toast_until = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notif(id: u32, app: &str) -> Event {
        Event::Notification(NotificationEvent { id, app: app.into(), title: format!("t{id}"), body: None, icon: None, aumid: None })
    }

    #[test]
    fn toast_shows_with_mail_icon_and_expires() {
        let t0 = Instant::now();
        let mut m = NotificationsModule::default();
        assert!(m.on_event(&notif(1, "Outlook"), t0));
        let v = m.view(t0).unwrap();
        assert_eq!(v.compact.icon, Icon::Mail);
        m.on_deadline(t0 + TOAST_VISIBLE);
        assert!(m.view(t0 + TOAST_VISIBLE).is_none());
    }

    #[test]
    fn toast_carries_app_body_and_icon_and_click_opens_the_app() {
        let t0 = Instant::now();
        let mut m = NotificationsModule::default();
        let n = NotificationEvent {
            id: 7,
            app: "Outlook".into(),
            title: "Prof. Ana".into(),
            body: Some("Enunciado no Moodle".into()),
            icon: None,
            aumid: Some("Microsoft.Office.OUTLOOK.EXE.15".into()),
        };
        m.on_event(&Event::Notification(n), t0);
        let toast = m.view(t0).unwrap().compact.toast.unwrap();
        assert_eq!((toast.app.as_str(), toast.body.as_str()), ("Outlook", "Enunciado no Moodle"));
        let mut out = Vec::new();
        let click = Action { module: ID, control: toast.open, kind: ActionKind::Press };
        assert!(m.on_action(&click, t0, &mut out));
        assert_eq!(out, [Command::OpenApp { aumid: "Microsoft.Office.OUTLOOK.EXE.15".into() }]);
        assert!(m.view(t0).is_none(), "o toast fecha ao abrir a app");
    }

    #[test]
    fn dedup_and_bounded_history() {
        let t0 = Instant::now();
        let mut m = NotificationsModule::default();
        assert!(!{
            m.on_event(&notif(1, "Teams"), t0);
            m.on_event(&notif(1, "Teams"), t0)
        });
        for i in 2..100 {
            m.on_event(&notif(i, "Teams"), t0);
        }
        assert_eq!(m.recent.len(), HISTORY);
    }
}
