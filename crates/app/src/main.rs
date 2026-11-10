// Em release no Windows, sem janela de consola.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use island_core::Engine;
use std::time::Instant;

fn main() -> std::io::Result<()> {
    let engine = Engine::new(island_modules::default_modules(), Instant::now());

    let args: Vec<String> = std::env::args().collect();
    let headless = args.iter().any(|a| a == "--headless");

    #[cfg(windows)]
    if !headless {
        // `--stdin`: também lê mensagens do protocolo pelo stdin (desenvolvimento).
        let dev_stdin = args.iter().any(|a| a == "--stdin");
        return island_platform::win::run(engine, dev_stdin);
    }

    if !headless {
        eprintln!("A janela só existe no Windows; a correr em modo --headless.");
    }
    island_platform::headless::run(engine)
}
