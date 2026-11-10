//! `island-hook`: ponte entre o Claude Code e a island.
//!
//! Três modos, pelo primeiro argumento:
//! - **(nenhum)**: hook de um evento do Claude Code (JSON no stdin) -> island, pelo pipe.
//! - **`statusline`**: linha de estado do Claude Code. Recebe o JSON com `rate_limits`, manda os
//!   limites da conta à island, guarda o último valor em `%LOCALAPPDATA%\dynamic-island\usage.json`
//!   (para a island os mostrar ao arrancar) e imprime uma linha curta ("5h 29% | 7d 55%").
//! - **`settings install|uninstall`**: junta ou tira os hooks e a linha de estado ao
//!   `~/.claude/settings.json` (ver `settings.rs`). Usado pelo instalador.
//!
//! Regras inquebráveis nos dois primeiros modos:
//! - **Sair sempre com código 0 e depressa.** Um hook que falha ou demora atrasa o
//!   Claude. Se a island não estiver a correr, não faz nada (nunca fica à espera do pipe).
//! - No hook, nunca escrever no stdout (no `Stop`/`PreToolUse` pode ser interpretado pelo Claude
//!   Code); só a linha de estado escreve (é para isso). Diagnóstico só com `ISLAND_HOOK_DEBUG=1`.
#![forbid(unsafe_code)]

mod settings;

use island_proto::statusline::{USAGE_FILE, state_dir, status_line_text, usage_from_status_line};
use island_proto::{Agent, ClaudeHook, ClaudeUsage, Envelope, HookEvent, Message};
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// O payload dos hooks é pequeno; isto só existe para nunca ler algo absurdo.
const MAX_STDIN: u64 = 1024 * 1024;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("settings") {
        // Este modo é do instalador, não de um hook: devolve o seu próprio código de saída.
        std::process::exit(settings::cli(&args[1..]));
    }
    let debug = std::env::var_os("ISLAND_HOOK_DEBUG").is_some();
    let result = if args.first().map(String::as_str) == Some("statusline") { run_statusline() } else { run() };
    if let Err(e) = result
        && debug
    {
        let _ = writeln!(std::io::stderr(), "island-hook: {e}");
    }
    // Código de saída 0 sempre, mesmo em erro.
}

/// Lê o stdin (limitado) como JSON, tolerando o BOM do PowerShell.
fn read_json() -> Result<Value, Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::stdin().take(MAX_STDIN).read_to_string(&mut input)?;
    Ok(serde_json::from_str(input.trim_start_matches('\u{feff}'))?)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let value = read_json()?;
    let Some(hook) = map_hook(&value, agent_from_args(std::env::args().skip(1))) else { return Ok(()) };
    send(&island_proto::encode(&Envelope::new(Message::ClaudeHook(hook))))
}

/// Linha de estado: limites -> island (pipe) e -> disco; e uma linha para o Claude Code mostrar.
fn run_statusline() -> Result<(), Box<dyn std::error::Error>> {
    let Some(usage) = usage_from_status_line(&read_json()?) else { return Ok(()) };
    // A linha para o Claude Code primeiro: não depende do disco nem do pipe.
    let _ = writeln!(std::io::stdout(), "{}", status_line_text(&usage));
    // O disco antes do pipe: com a island fechada o pipe falha logo, e o valor fica guardado.
    let saved = state_dir().map(|dir| save_usage(&dir, &usage));
    let sent = send(&island_proto::encode(&Envelope::new(Message::ClaudeUsage(usage))));
    // Se ao menos um dos dois funcionou, não há erro a reportar (a island fechada é normal).
    match (saved, sent) {
        (Some(Err(e)), Err(_)) => Err(e),
        _ => Ok(()),
    }
}

/// Grava o último valor em `<dir>\usage.json`: escreve num temporário e troca, para a island
/// nunca ler meio ficheiro.
fn save_usage(dir: &Path, usage: &ClaudeUsage) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{USAGE_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(usage)?)?;
    std::fs::rename(&tmp, dir.join(USAGE_FILE)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Converte o JSON do Claude Code no nosso formato (só o que a island usa).
fn map_hook(v: &Value, agent: Agent) -> Option<ClaudeHook> {
    let str_field = |k: &str| v.get(k).and_then(Value::as_str);
    let event = HookEvent::from_hook_name(str_field("hook_event_name")?);
    if event == HookEvent::Other || is_idle_reminder(v) {
        return None;
    }
    let project = str_field("cwd").and_then(|cwd| {
        // Aceita separadores Windows e Unix.
        cwd.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().map(str::to_owned)
    });
    // O modelo e os tokens do contexto vêm do fim do ficheiro da conversa (o Claude Code guarda lá
    // o uso de cada resposta). Só nos eventos em que vale a pena, e nunca a falhar o hook.
    let (context_tokens, model) = match (event, str_field("transcript_path")) {
        (HookEvent::Notification | HookEvent::SessionEnd, _) | (_, None) => (None, None),
        (_, Some(path)) => last_usage(path).map_or((None, None), |(t, m)| (Some(t), Some(m))),
    };
    Some(ClaudeHook {
        agent,
        event,
        session_id: str_field("session_id")?.to_owned(),
        project: project.filter(|p| !p.is_empty()),
        tool: str_field("tool_name").map(str::to_owned),
        message: str_field("message").map(str::to_owned),
        model,
        context_tokens,
    })
}

/// Quanto do fim do ficheiro da conversa se lê. Chega para a última resposta (uma linha por
/// mensagem) e mantém o hook rápido num ficheiro de dezenas de MB.
const TAIL_BYTES: u64 = 768 * 1024;

/// Tokens do contexto (entrada + cache) e modelo da última resposta do assistente, lidos do
/// fim do ficheiro `.jsonl` da conversa. `None` se não há (ficheiro novo, sem permissão...).
fn last_usage(path: &str) -> Option<(u64, String)> {
    // Só ficheiros de conversa: o caminho vem do Claude Code, mas não se lê qualquer coisa.
    if !path.ends_with(".jsonl") {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES))).ok()?;
    let mut tail = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut tail).ok()?;
    usage_from_tail(&String::from_utf8_lossy(&tail))
}

/// A parte testável: a última linha de assistente com `usage`. A 1.ª linha do pedaço pode estar
/// cortada ao meio e simplesmente não faz parse.
fn usage_from_tail(tail: &str) -> Option<(u64, String)> {
    for line in tail.lines().rev() {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let message = v.get("message")?;
        let model = message.get("model").and_then(Value::as_str)?;
        // Respostas sintéticas (erros locais) não têm uso real.
        if model.starts_with('<') {
            continue;
        }
        let usage = message.get("usage")?;
        let field = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
        let tokens = field("input_tokens") + field("cache_creation_input_tokens") + field("cache_read_input_tokens");
        return Some((tokens, model.to_owned()));
    }
    None
}

/// `--agent codex` (ou `--agent=codex`) diz que o evento vem do Codex. Sem a opção, é o Claude.
fn agent_from_args(args: impl Iterator<Item = String>) -> Agent {
    let mut args = args;
    while let Some(a) = args.next() {
        if let Some(name) = a.strip_prefix("--agent=") {
            return Agent::from_name(name);
        }
        if a == "--agent" {
            return args.next().map_or(Agent::Claude, |n| Agent::from_name(&n));
        }
    }
    Agent::Claude
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
        let h = map_hook(&v, Agent::Claude).unwrap();
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
        let h = map_hook(&v, Agent::Claude).unwrap();
        assert_eq!(h.project.as_deref(), Some("island"));
        assert!(h.message.unwrap().contains("permission"));
    }

    #[test]
    fn idle_reminders_are_not_requests() {
        let by_type = json!({
            "session_id": "a", "cwd": "C:\\x\\p", "hook_event_name": "Notification",
            "notification_type": "idle_prompt", "message": "Claude is waiting for your input"
        });
        assert!(map_hook(&by_type, Agent::Claude).is_none());
        let old_version = json!({
            "session_id": "a", "hook_event_name": "Notification", "message": "Claude is waiting for your input"
        });
        assert!(map_hook(&old_version, Agent::Claude).is_none());
        let permission = json!({
            "session_id": "a", "hook_event_name": "Notification",
            "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"
        });
        assert!(map_hook(&permission, Agent::Claude).is_some());
    }

    #[test]
    fn ignores_unknown_events_and_garbage() {
        assert!(map_hook(&json!({"session_id": "a", "hook_event_name": "PreCompact"}), Agent::Claude).is_none());
        assert!(map_hook(&json!({"hook_event_name": "Stop"}), Agent::Claude).is_none(), "sem session_id");
        assert!(map_hook(&json!([]), Agent::Claude).is_none());
    }

    #[test]
    fn agent_option_selects_codex() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        assert_eq!(agent_from_args(args(&[])), Agent::Claude);
        assert_eq!(agent_from_args(args(&["--agent", "codex"])), Agent::Codex);
        assert_eq!(agent_from_args(args(&["--agent=Codex"])), Agent::Codex);
        assert_eq!(agent_from_args(args(&["--agent"])), Agent::Claude);
        let v = json!({"session_id": "s", "hook_event_name": "Stop"});
        assert_eq!(map_hook(&v, Agent::Codex).unwrap().agent, Agent::Codex);
    }

    #[test]
    fn reads_context_and_model_from_the_last_assistant_line() {
        let tail = [
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","usage":{"input_tokens":4,"cache_creation_input_tokens":405,"cache_read_input_tokens":431907,"output_tokens":9}}}"#,
            r#"{"type":"user","message":{"content":"olá"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-sonnet-5-5","usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":1000}}}"#,
            r#"{"type":"user","message":{"content":"outra"}}"#,
        ]
        .join("\n");
        assert_eq!(usage_from_tail(&tail), Some((1010, "claude-sonnet-5-5".into())));
    }

    #[test]
    fn a_cut_first_line_and_synthetic_replies_are_skipped() {
        let tail = [
            r#"cortada ao meio "usage": {"x":1}}}"#,
            r#"{"message":{"model":"claude-opus-5-5","usage":{"input_tokens":7}}}"#,
            r#"{"message":{"model":"<synthetic>","usage":{"input_tokens":0}}}"#,
        ]
        .join("\n");
        assert_eq!(usage_from_tail(&tail), Some((7, "claude-opus-5-5".into())));
        assert_eq!(usage_from_tail("sem nada útil\n{}"), None);
    }

    #[test]
    fn only_conversation_files_are_read() {
        assert_eq!(last_usage(r"C:\Windows\win.ini"), None);
        assert_eq!(last_usage(r"C:\nao\existe.jsonl"), None);
    }

    #[test]
    fn saved_usage_round_trips_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("island-usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let usage = ClaudeUsage {
            five_hour: Some(island_proto::UsageWindow { used_pct: 29.0, resets_at: Some(1_794_366_000) }),
            seven_day: None,
        };
        save_usage(&dir, &usage).unwrap();
        // Uma segunda gravação substitui a primeira.
        let newer = ClaudeUsage { seven_day: Some(island_proto::UsageWindow { used_pct: 55.0, resets_at: None }), ..usage };
        save_usage(&dir, &newer).unwrap();
        let back: ClaudeUsage = serde_json::from_slice(&std::fs::read(dir.join(USAGE_FILE)).unwrap()).unwrap();
        assert_eq!(back, newer);
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [USAGE_FILE], "o temporário não fica");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
