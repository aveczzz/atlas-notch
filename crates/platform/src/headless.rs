//! Backend sem janela, para desenvolvimento em qualquer SO.
//!
//! ```text
//! echo '{"v":1,"msg":{"type":"show","source":"x","title":"Olá","ttl_ms":2000}}' | cargo run -p island -- --headless
//! ```
//!
//! É também a referência do event loop: o backend Windows faz exatamente isto,
//! trocando `recv_timeout` por `MsgWaitForMultipleObjectsEx`.

use crate::EventProxy;
use island_core::{Engine, Event, Presented};
use std::io::{self, BufRead, Read, Write};
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Instant;

pub fn run(mut engine: Engine) -> io::Result<()> {
    let (tx, rx) = crate::proxy::channel();
    let proxy = EventProxy::new(tx, Arc::new(|| {}));

    // Fonte: stdin com o mesmo protocolo do named pipe.
    thread::Builder::new().name("stdin-ipc".into()).spawn(move || read_ipc(io::stdin().lock(), &proxy))?;

    let mut out = io::stdout().lock();
    print_view(&mut out, engine.current())?;

    loop {
        // Dormir até ao próximo evento ou deadline. Sem deadlines: dorme indefinidamente.
        let received = match engine.next_deadline() {
            Some(deadline) => rx.recv_timeout(deadline.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let now = Instant::now();
        let redraw = match received {
            Ok(event) => engine.dispatch(event, now),
            Err(RecvTimeoutError::Timeout) => engine.on_deadline(now),
            Err(RecvTimeoutError::Disconnected) => {
                // stdin fechou: deixar expirar o que falta e sair.
                match engine.next_deadline() {
                    Some(d) => {
                        thread::sleep(d.saturating_duration_since(now));
                        engine.on_deadline(Instant::now())
                    }
                    None => break,
                }
            }
        };
        for cmd in engine.drain_commands() {
            writeln!(out, "  -> comando: {cmd:?}")?;
        }
        if redraw {
            print_view(&mut out, engine.current())?;
        }
    }
    Ok(())
}

/// Lê linhas do protocolo e envia-as como eventos. Partilhado com o servidor de pipe.
///
/// Cada linha é lida com teto (`MAX_FRAME_BYTES`): um cliente que nunca manda `\n` não
/// faz crescer a memória; a linha comprida é descartada até ao fim.
pub fn read_ipc(mut reader: impl BufRead, proxy: &EventProxy) {
    let limit = island_proto::MAX_FRAME_BYTES as u64 + 1;
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader).take(limit).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() as u64 >= limit && line.last() != Some(&b'\n') {
            log!("ipc: linha com mais de {} bytes descartada", island_proto::MAX_FRAME_BYTES);
            if reader.skip_until(b'\n').is_err() {
                break;
            }
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match island_proto::decode_line(&line) {
            Ok(env) => {
                if !proxy.send(Event::External(env.msg)) {
                    break;
                }
            }
            Err(e) => log!("ipc: {e}"),
        }
    }
}

/// Uma linha com a view atual. Também serve de log no backend Windows até haver renderer.
pub(crate) fn print_view(out: &mut impl Write, p: Option<&Presented>) -> io::Result<()> {
    match p {
        None => writeln!(out, "[ island vazia ]"),
        Some(p) => {
            let c = &p.view.compact;
            let progress = c.progress.map(|v| format!(" {:.0}%", v * 100.0)).unwrap_or_default();
            let pulse = if c.pulse { " (pulsa)" } else { "" };
            writeln!(
                out,
                "[ {:?} | {:?} {}{}{} ]  <{}>",
                p.view.priority, c.icon, c.text, progress, pulse, p.module
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_proto::MAX_FRAME_BYTES;

    #[test]
    fn oversized_lines_are_skipped_without_losing_the_next_one() {
        let (tx, rx) = crate::proxy::channel();
        let proxy = EventProxy::new(tx, Arc::new(|| {}));
        let mut input = vec![b'x'; MAX_FRAME_BYTES * 3];
        input.extend_from_slice(b"\n{\"v\":1,\"msg\":{\"type\":\"clear\",\"source\":\"ok\"}}\n");
        read_ipc(io::Cursor::new(input), &proxy);
        let got: Vec<_> = rx.try_iter().collect();
        assert_eq!(got, vec![Event::External(island_proto::Message::Clear { source: "ok".into() })]);
    }
}
