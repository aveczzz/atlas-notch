//! `island-hook`: ponte entre os hooks do Claude Code e a island.
//!
//! O Claude Code executa isto em cada evento configurado, com o JSON do evento no
//! stdin. Regras inquebráveis:
//! - **Sair sempre com código 0 e depressa.** Um hook que falha ou demora atrasa o
//!   Claude. Se a island não estiver a correr, não faz nada.
//! - Nunca escrever no stdout (no `Stop`/`PreToolUse` o stdout pode ser interpretado
//!   pelo Claude Code). Diagnóstico só com `ISLAND_HOOK_DEBUG=1`, para stderr.
#![forbid(unsafe_code)]

use island_proto::{ClaudeHook, Envelope, HookEvent, Message};
use serde_json::Value;
use std::io::{Read, Write};

/// O payload dos hooks é pequeno; isto só existe para nunca ler algo absurdo.
const MAX_STDIN: u64 = 1024 * 1024;

fn main() {
    let debug = std::env::var_os("ISLAND_HOOK_DEBUG").is_some();
    if let Err(e) = run()
        && debug
    {
        let _ = writeln!(std::io::stderr(), "island-hook: {e}");
    }
    // Código de saída 0 sempre, mesmo em erro.
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::stdin().take(MAX_STDIN).read_to_string(&mut input)?;
    // O PowerShell (e outros clientes .NET) põe um BOM UTF-8 antes do JSON.
    let value: Value = serde_json::from_str(input.trim_start_matches('\u{feff}'))?;
    let Some(hook) = map_hook(&value) else { return Ok(()) };
    send(&island_proto::encode(&Envelope::new(Message::ClaudeHook(hook))))
}

/// Converte o JSON do Claude Code no nosso formato (só o que a island usa).
fn map_hook(v: &Value) -> Option<ClaudeHook> {
    let str_field = |k: &str| v.get(k).and_then(Value::as_str);
    let event = HookEvent::from_hook_name(str_field("hook_event_name")?);
    if event == HookEvent::Other || is_idle_reminder(v) {
        return None;
    }
    let project = str_field("cwd").and_then(|cwd| {
        // Aceita separadores Windows e Unix.
        cwd.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().map(str::to_owned)
    });
    Some(ClaudeHook {
        event,
        session_id: str_field("session_id")?.to_owned(),
        project: project.filter(|p| !p.is_empty()),
        tool: str_field("tool_name").map(str::to_owned),
        message: str_field("message").map(str::to_owned),
        model: None,
        context_tokens: None,
    })
}

/// O Claude Code também manda `Notification` quando está parado à espera de um prompt novo
/// (~60 s depois do `Stop`). Isso não é "precisa de ti": a sessão já está como "terminou", e
/// tratá-lo como pedido pôs cada sessão acabada a pulsar durante até 30 minutos.
/// Versões recentes mandam `notification_type`; as antigas só a mensagem.
fn is_idle_reminder(v: &Value) -> bool {
    if let Some(kind) = v.get("notification_type").and_then(Value::as_str) {
        return kind == "idle_prompt";
    }
    v.get("message").and_then(Value::as_str).is_some_and(|m| m.contains("waiting for your input"))
}

#[cfg(windows)]
fn send(frame: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{ErrorKind, Write};
    use std::time::Duration;
    // Abrir um named pipe como ficheiro é suportado pela std.
    // - Pipe não existe (island fechada): desistir logo.
    // - Pipe ocupado (ERROR_PIPE_BUSY, outro hook a escrever): tentar mais 2 vezes.
    //   No pior caso o hook demora ~40 ms, e o Claude nunca fica à espera de mais.
    let mut attempt = 0;
    let mut pipe = loop {
        match std::fs::OpenOptions::new().write(true).open(island_proto::PIPE_NAME) {
            Ok(p) => break p,
            Err(e) if e.kind() == ErrorKind::NotFound || attempt == 2 => return Err(e.into()),
            Err(_) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    pipe.write_all(frame)?;
    Ok(())
}

#[cfg(not(windows))]
fn send(frame: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    // Fora do Windows (desenvolvimento): envia para o stderr, para poder fazer
    // `island-hook < evento.json 2>&1 | island --headless`.
    use std::io::Write;
    std::io::stderr().write_all(frame)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_pre_tool_use_with_windows_path() {
        let v = json!({
            "session_id": "abc",
            "transcript_path": "C:\\Users\\r\\.claude\\x.jsonl",
            "cwd": "C:\\Users\\r\\code\\activesync\\",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "cargo build"}
        });
        let h = map_hook(&v).unwrap();
        assert_eq!(h.event, HookEvent::PreToolUse);
        assert_eq!(h.project.as_deref(), Some("activesync"));
        assert_eq!(h.tool.as_deref(), Some("Bash"));
    }

    #[test]
    fn maps_notification_message() {
        let v = json!({
            "session_id": "abc",
            "cwd": "/home/r/island",
            "hook_event_name": "Notification",
            "message": "Claude needs your permission to use Bash"
        });
        let h = map_hook(&v).unwrap();
        assert_eq!(h.project.as_deref(), Some("island"));
        assert!(h.message.unwrap().contains("permission"));
    }

    #[test]
    fn idle_reminders_are_not_requests() {
        let by_type = json!({
            "session_id": "a", "cwd": "C:\\x\\p", "hook_event_name": "Notification",
            "notification_type": "idle_prompt", "message": "Claude is waiting for your input"
        });
        assert!(map_hook(&by_type).is_none());
        let old_version = json!({
            "session_id": "a", "hook_event_name": "Notification", "message": "Claude is waiting for your input"
        });
        assert!(map_hook(&old_version).is_none());
        let permission = json!({
            "session_id": "a", "hook_event_name": "Notification",
            "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"
        });
        assert!(map_hook(&permission).is_some());
    }

    #[test]
    fn ignores_unknown_events_and_garbage() {
        assert!(map_hook(&json!({"session_id": "a", "hook_event_name": "PreCompact"})).is_none());
        assert!(map_hook(&json!({"hook_event_name": "Stop"})).is_none(), "sem session_id");
        assert!(map_hook(&json!([])).is_none());
    }
}
