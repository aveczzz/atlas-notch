//! Log da island: stderr (se alguém o estiver a ler) e o ficheiro `island.log` em
//! `%APPDATA%\dynamic-island`.
//!
//! A app corre sem consola, por isso o stderr perde-se: sem ficheiro, uma falha no arranque do
//! Windows não deixava rasto nenhum. O ficheiro é pequeno (passa a `island.log.old` aos 256 KB)
//! e **nunca** falha para fora: um erro a escrever o log é ignorado, como no `log!`.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 256 * 1024;
const FILE_NAME: &str = "island.log";

static FILE: Mutex<Option<File>> = Mutex::new(None);

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    let dir = PathBuf::from(base).join("dynamic-island");
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join(FILE_NAME))
}

fn open() -> Option<File> {
    let path = path()?;
    if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(&path, path.with_extension("log.old"));
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// `HH:MM:SS` em UTC (sem fusos: bastam para ordenar o que aconteceu).
fn clock(secs: u64) -> String {
    let day = secs % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, day % 3600 / 60, day % 60)
}

/// O que o `log!` chama. Nunca entra em pânico: o `eprintln!` rebentava com o stderr fechado.
pub fn write(args: fmt::Arguments<'_>) {
    let _ = writeln!(std::io::stderr(), "{args}");
    // Os testes não escrevem no log verdadeiro do utilizador.
    if cfg!(test) {
        return;
    }
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // Um pânico noutra thread com o lock tomado não pode calar os logs seguintes.
    let mut guard = FILE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = open();
    }
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "[{} UTC] {args}", clock(secs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_time_of_day() {
        assert_eq!(clock(0), "00:00:00");
        assert_eq!(clock(86_400 + 3_661), "01:01:01");
    }
}
