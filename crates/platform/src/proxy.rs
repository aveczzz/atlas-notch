use island_core::Event;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

/// Eventos à espera da thread UI, no máximo. A thread UI drena tudo a cada wakeup, por isso
/// encher isto significa uma fonte descontrolada: mais vale perder eventos do que memória.
pub const QUEUE: usize = 256;

/// Canal limitado entre as fontes e a thread UI.
pub fn channel() -> (SyncSender<Event>, Receiver<Event>) {
    sync_channel(QUEUE)
}

/// Ponte entre threads de fontes e a thread UI.
///
/// Envia o evento para o canal e "acorda" a thread UI. No Windows o `wake` faz
/// `PostMessageW(hwnd, WM_APP_WAKE, ..)`; no headless não é preciso (o loop
/// bloqueia diretamente no canal).
///
/// Nunca bloqueia (os callbacks COM/WinRT chamam isto): com o canal cheio, o evento perde-se.
#[derive(Clone)]
pub struct EventProxy {
    tx: SyncSender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for EventProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventProxy")
    }
}

impl EventProxy {
    pub fn new(tx: SyncSender<Event>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { tx, wake }
    }

    /// Devolve `false` se a thread UI já terminou (a fonte deve parar).
    pub fn send(&self, event: Event) -> bool {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => log!("eventos: fila cheia, evento descartado"),
            Err(TrySendError::Disconnected(_)) => return false,
        }
        (self.wake)();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_proto::Message;

    #[test]
    fn full_queue_drops_instead_of_blocking() {
        let (tx, rx) = channel();
        let proxy = EventProxy::new(tx, Arc::new(|| {}));
        let ev = || Event::External(Message::Clear { source: "x".into() });
        for _ in 0..QUEUE + 10 {
            assert!(proxy.send(ev()));
        }
        assert_eq!(rx.try_iter().count(), QUEUE);
        drop(rx);
        assert!(!proxy.send(ev()), "UI terminou: a fonte deve parar");
    }
}
