//! Limites de uso da conta (5 horas e semana): o último valor guardado pelo `island-hook
//! statusline` em `%LOCALAPPDATA%\dynamic-island\usage.json`.
//!
//! A island só lê este ficheiro **ao arrancar** (para mostrar o último valor mesmo que tenha
//! estado fechada). Com ela aberta, os valores chegam ao vivo pelo pipe (`claude_usage`). O módulo
//! do Claude Code considera uma janela expirada quando passa o `resets_at`, por isso um ficheiro
//! velho não mostra números que já não valem.

use island_proto::ClaudeUsage;
use island_proto::statusline::{USAGE_FILE, state_dir};
use std::path::Path;

/// Um ficheiro enorme não é nosso: a leitura nunca passa disto.
const MAX_BYTES: u64 = 64 * 1024;

/// O último valor guardado, ou `None` (não existe, está estragado, ou não tem nenhuma janela).
pub fn load_saved() -> Option<ClaudeUsage> {
    load(&state_dir()?)
}

fn load(dir: &Path) -> Option<ClaudeUsage> {
    let path = dir.join(USAGE_FILE);
    if std::fs::metadata(&path).ok()?.len() > MAX_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let usage: ClaudeUsage = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    (usage.five_hour.is_some() || usage.seven_day.is_some()).then_some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_proto::UsageWindow;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("island-load-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_what_the_hook_saved() {
        let dir = temp_dir("ok");
        let saved = ClaudeUsage { five_hour: Some(UsageWindow { used_pct: 29.0, resets_at: Some(1_794_366_000) }), seven_day: None };
        std::fs::write(dir.join(USAGE_FILE), serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(load(&dir), Some(saved));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_half_written_or_empty_files_give_nothing() {
        let dir = temp_dir("bad");
        assert_eq!(load(&dir), None, "não existe");
        std::fs::write(dir.join(USAGE_FILE), r#"{"five_hour":{"used_p"#).unwrap();
        assert_eq!(load(&dir), None, "a meio");
        std::fs::write(dir.join(USAGE_FILE), "{}").unwrap();
        assert_eq!(load(&dir), None, "sem janelas");
        let _ = std::fs::remove_dir_all(dir);
    }
}
