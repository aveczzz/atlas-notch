//! Camada de plataforma. É o único crate onde há `unsafe` e chamadas ao sistema.
//!
//! Dois backends com o mesmo modelo de event loop:
//! - `headless`: corre em qualquer SO, lê mensagens IPC do stdin e imprime a view.
//!   Serve para desenvolver e testar os módulos sem Windows.
//! - `win`: a app real.
//!
//! Modelo de threads (igual nos dois):
//!
//! ```text
//! thread UI (única dona do Engine e do renderer)
//!   ^  Event via mpsc + "wake" (PostMessage no Windows)
//!   |
//! threads/callbacks de fontes: pipe IPC, GSMTC, Core Audio, notificações
//! ```
//!
//! Nenhuma fonte faz polling. A thread UI dorme até chegar um evento, expirar um
//! deadline de um módulo, ou ser preciso o próximo frame de uma animação.

/// Como o `eprintln!`, mas **nunca entra em pânico**. O `eprintln!` rebenta se o stderr for
/// um pipe fechado (ex.: lançada por um script que já terminou) e, com `panic = "abort"`,
/// fechava a island. Aqui um erro de escrita é ignorado: o log perde-se, a app continua.
macro_rules! log {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

pub mod config;
pub mod headless;
mod proxy;
#[cfg(windows)]
pub mod win;

pub use proxy::{EventProxy, QUEUE};
