//! Fonte de notificações: `UserNotificationListener`.
//!
//! **Requer identidade de pacote** (o sparse package em `packaging/sparse/`, instalado com
//! `packaging/instalar.ps1`). Sem ela, `RequestAccessAsync` falha: a fonte desliga-se e a
//! island continua sem notificações.
//!
//! Uma thread `notifications` é dona do listener:
//! - `RequestAccessAsync()` uma vez (na primeira vez o Windows pergunta ao utilizador).
//! - Mudanças: o evento `NotificationChanged`, quando o Windows o aceita. Se não aceitar,
//!   consulta a lista a cada `POLL` — a única espera periódica da app, documentada no
//!   CLAUDE.md, e só existe com acesso concedido.
//! - Novas = ids que não estavam na lista anterior. As que já existiam ao arrancar não
//!   aparecem (seria uma enxurrada).
//! - De cada `UserNotification`: `AppInfo().DisplayInfo().DisplayName()` e, do binding
//!   `ToastGeneric`, os textos (1.º = título, 2.º = corpo).
//! - `RemoveNotification(id)` quando o utilizador dispensa na island (comando do `exec`).

use super::appicon;
use crate::EventProxy;
use island_core::Event;
use island_core::event::{ImageData, NotificationEvent};
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread;
use std::time::Duration;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::Result;

/// Intervalo de consulta quando o evento não está disponível.
const POLL: Duration = Duration::from_secs(2);
/// Ids lembrados, no máximo (regra 6). O Windows guarda poucas dezenas.
const MAX_SEEN: usize = 256;
/// Apps com ícone em cache, no máximo.
const MAX_ICONS: usize = 32;

#[derive(Debug)]
enum Msg {
    Changed,
    Dismiss(u32),
}

#[derive(Debug, Clone)]
pub struct NotificationControl(SyncSender<Msg>);

impl NotificationControl {
    pub fn dismiss(&self, id: u32) {
        let _ = self.0.try_send(Msg::Dismiss(id));
    }
}

pub fn spawn(proxy: EventProxy) -> io::Result<NotificationControl> {
    let (tx, rx) = sync_channel(32);
    let control = NotificationControl(tx.clone());
    thread::Builder::new().name("notifications".into()).spawn(move || {
        if let Err(e) = run(&rx, &tx, &proxy) {
            // Esperado sem o sparse package: não há identidade, não há acesso.
            log!("notificações desligadas: {e}");
        }
    })?;
    Ok(control)
}

fn run(rx: &Receiver<Msg>, tx: &SyncSender<Msg>, proxy: &EventProxy) -> Result<()> {
    // SAFETY: COM (MTA) nesta thread, dona do listener.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
    let listener = UserNotificationListener::Current()?;
    let status = listener.RequestAccessAsync()?.join()?;
    if status != UserNotificationListenerAccessStatus::Allowed {
        log!("notificações: acesso {status:?} (ver Definições > Privacidade > Notificações)");
        return Ok(());
    }
    let t = tx.clone();
    let event = listener.NotificationChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.try_send(Msg::Changed);
        Ok(())
    }));
    let poll = event.is_err();
    if poll {
        log!("notificações: sem evento NotificationChanged, a consultar a cada {POLL:?}");
    }

    // Estado inicial: lembrar o que já lá está, sem o mostrar.
    let mut seen: HashSet<u32> = current(&listener)?.iter().filter_map(|n| n.Id().ok()).collect();
    // Ícones já obtidos, por app: o Shell demora dezenas de ms e a mesma app notifica muitas vezes.
    let mut icons: HashMap<String, Option<ImageData>> = HashMap::new();
    loop {
        let msg = if poll { rx.recv_timeout(POLL) } else { rx.recv().map_err(|_| RecvTimeoutError::Disconnected) };
        match msg {
            Ok(Msg::Dismiss(id)) => {
                let _ = listener.RemoveNotification(id);
                continue;
            }
            Ok(Msg::Changed) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let list = match current(&listener) {
            Ok(list) => list,
            Err(e) => {
                log!("notificações: {e}");
                continue;
            }
        };
        let mut now_seen = HashSet::with_capacity(list.len());
        for n in &list {
            let Ok(id) = n.Id() else { continue };
            now_seen.insert(id);
            if !seen.contains(&id)
                && let Some(event) = describe(n, id, &mut icons)
                && !proxy.send(Event::Notification(event))
            {
                return Ok(());
            }
        }
        // Só a lista atual conta: as dispensadas saem, e o conjunto nunca cresce sem limite.
        seen = now_seen;
        if seen.len() > MAX_SEEN {
            seen.clear();
        }
    }
    if let Ok(token) = event {
        let _ = listener.RemoveNotificationChanged(token);
    }
    Ok(())
}

fn current(listener: &UserNotificationListener) -> Result<Vec<UserNotification>> {
    let list = listener.GetNotificationsAsync(NotificationKinds::Toast)?.join()?;
    (0..list.Size()?).map(|i| list.GetAt(i)).collect()
}

/// App, título e corpo de uma notificação (toast genérico). `None` se não tiver texto.
fn describe(n: &UserNotification, id: u32, icons: &mut HashMap<String, Option<ImageData>>) -> Option<NotificationEvent> {
    let app = n.AppInfo().and_then(|a| a.DisplayInfo()).and_then(|d| d.DisplayName()).map(|s| s.to_string_lossy());
    let binding = n.Notification().ok()?.Visual().ok()?.GetBinding(&KnownNotificationBindings::ToastGeneric().ok()?).ok()?;
    let texts = binding.GetTextElements().ok()?;
    let text = |i: u32| texts.GetAt(i).and_then(|t| t.Text()).map(|s| s.to_string_lossy()).ok();
    let title = text(0).filter(|t| !t.trim().is_empty())?;
    let aumid = n.AppInfo().and_then(|a| a.AppUserModelId()).map(|s| s.to_string_lossy()).ok().filter(|s| !s.is_empty());
    let icon = aumid.as_ref().and_then(|id| {
        if icons.len() >= MAX_ICONS {
            icons.clear();
        }
        icons.entry(id.clone()).or_insert_with(|| appicon::for_app(id)).clone()
    });
    Some(NotificationEvent {
        id,
        app: app.unwrap_or_default(),
        title,
        body: text(1).filter(|b| !b.trim().is_empty()),
        icon,
        aumid,
    })
}
