//! Protocolo entre a island e o mundo exterior (hook do Claude Code, scripts, outras apps).
//!
//! Formato: JSON delimitado por newline (uma mensagem por linha), sobre um named pipe local.
//! É propositadamente simples: qualquer linguagem consegue falar isto com 5 linhas de código.
//!
//! ```text
//! {"v":1,"msg":{"type":"show","source":"backup","title":"Veeam OK","priority":"info","ttl_ms":5000}}
//! ```
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::fmt;

/// Nome do named pipe. Só aceita clientes locais (ver CLAUDE.md, secção Segurança).
pub const PIPE_NAME: &str = r"\\.\pipe\dynamic-island";

/// Versão atual do protocolo. Incrementar só em mudanças incompatíveis.
pub const PROTOCOL_VERSION: u32 = 1;

/// Tamanho máximo de uma linha. Protege o servidor contra clientes mal comportados.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

/// Limites de texto aplicados na receção, para o layout nunca rebentar.
pub const MAX_TITLE_CHARS: usize = 80;
pub const MAX_DETAIL_CHARS: usize = 200;
/// TTL máximo de um `show`. Para mostrar algo mais tempo, não mandar `ttl_ms` e usar `clear`.
pub const MAX_TTL_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u32,
    pub msg: Message,
}

impl Envelope {
    pub fn new(msg: Message) -> Self {
        Self { v: PROTOCOL_VERSION, msg }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Evento vindo de um hook do Claude Code (via `island-hook`).
    ClaudeHook(ClaudeHook),
    /// Qualquer app externa pede para mostrar algo. É o "sistema de plugins" da island.
    Show(Show),
    /// Limites de uso da conta (5 horas e semanal), vindos da linha de estado do Claude Code.
    ClaudeUsage(ClaudeUsage),
    /// Retira o que essa fonte estava a mostrar.
    Clear { source: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeHook {
    pub event: HookEvent,
    pub session_id: String,
    /// Nome da pasta do projeto (último componente do `cwd`). Usado para identificar
    /// a janela do VS Code certa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Modelo da última resposta (`claude-opus-5-5`), lido do ficheiro da conversa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tokens no contexto depois da última resposta (entrada + cache). Calculado pelo hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
}

/// Uso da conta nas duas janelas do plano. Cada uma pode faltar (planos sem limite, ou antes da
/// primeira resposta da sessão).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    /// Percentagem usada, 0 a 100.
    pub used_pct: f32,
    /// Quando a janela reinicia (segundos desde 1970).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaudeUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<UsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<UsageWindow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Notification,
    Stop,
    SubagentStop,
    SessionEnd,
    /// Eventos que ainda não conhecemos. Ignorados, mas não rebentam o parse.
    #[serde(other)]
    Other,
}

impl HookEvent {
    /// Converte o `hook_event_name` do Claude Code.
    pub fn from_hook_name(name: &str) -> Self {
        match name {
            "SessionStart" => Self::SessionStart,
            "UserPromptSubmit" => Self::UserPromptSubmit,
            "PreToolUse" => Self::PreToolUse,
            "PostToolUse" => Self::PostToolUse,
            "Notification" => Self::Notification,
            "Stop" => Self::Stop,
            "SubagentStop" => Self::SubagentStop,
            "SessionEnd" => Self::SessionEnd,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Show {
    /// Identificador estável da fonte ("backup", "deploy", ...). Uma fonte = um slot.
    pub source: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub priority: WirePriority,
    /// Se ausente, fica até receber `clear`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    /// 0.0 a 1.0, desenhado como barra de progresso.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
}

/// Prioridade no fio. Não permite `attention`: apps externas não podem roubar
/// a island ao Claude ou ao volume.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WirePriority {
    Ambient,
    #[default]
    Info,
    Active,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtoError {
    TooLarge(usize),
    UnsupportedVersion(u32),
    Malformed(String),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(n) => write!(f, "frame com {n} bytes excede {MAX_FRAME_BYTES}"),
            Self::UnsupportedVersion(v) => write!(f, "versão {v} não suportada"),
            Self::Malformed(e) => write!(f, "mensagem inválida: {e}"),
        }
    }
}

impl std::error::Error for ProtoError {}

/// Serializa para uma linha pronta a escrever no pipe (inclui o `\n`).
pub fn encode(env: &Envelope) -> Vec<u8> {
    // Serializar estes tipos não pode falhar (sem maps com chaves não-string).
    let mut out = serde_json::to_vec(env).expect("Envelope é sempre serializável");
    out.push(b'\n');
    out
}

/// Interpreta uma linha recebida e normaliza o conteúdo (corta textos longos).
pub fn decode_line(line: &[u8]) -> Result<Envelope, ProtoError> {
    if line.len() > MAX_FRAME_BYTES {
        return Err(ProtoError::TooLarge(line.len()));
    }
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    // Clientes .NET/PowerShell com `StreamWriter` mandam um BOM UTF-8 antes da 1.ª linha.
    let line = line.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(line);
    let mut env: Envelope =
        serde_json::from_slice(line).map_err(|e| ProtoError::Malformed(e.to_string()))?;
    if env.v != PROTOCOL_VERSION {
        return Err(ProtoError::UnsupportedVersion(env.v));
    }
    sanitize(&mut env.msg);
    Ok(env)
}

fn sanitize(msg: &mut Message) {
    match msg {
        Message::Show(s) => {
            truncate_chars(&mut s.source, 32);
            truncate_chars(&mut s.title, MAX_TITLE_CHARS);
            if let Some(d) = &mut s.detail {
                truncate_chars(d, MAX_DETAIL_CHARS);
            }
            s.progress = s.progress.filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0));
            s.ttl_ms = s.ttl_ms.map(|t| t.min(MAX_TTL_MS));
        }
        Message::ClaudeHook(h) => {
            truncate_chars(&mut h.session_id, 64);
            if let Some(p) = &mut h.project {
                truncate_chars(p, 48);
            }
            if let Some(t) = &mut h.tool {
                truncate_chars(t, 48);
            }
            if let Some(m) = &mut h.message {
                truncate_chars(m, MAX_DETAIL_CHARS);
            }
            if let Some(m) = &mut h.model {
                truncate_chars(m, 48);
            }
        }
        Message::ClaudeUsage(u) => {
            // Percentagens fora de 0..100 ou NaN nunca chegam ao desenho.
            for w in [&mut u.five_hour, &mut u.seven_day] {
                *w = w.filter(|w| w.used_pct.is_finite()).map(|w| UsageWindow { used_pct: w.used_pct.clamp(0.0, 100.0), ..w });
            }
        }
        Message::Clear { source } => truncate_chars(source, 32),
    }
}

/// Corta por caracteres (nunca a meio de um caracter UTF-8).
pub fn truncate_chars(s: &mut String, max: usize) {
    if let Some((idx, _)) = s.char_indices().nth(max) {
        s.truncate(idx);
        s.push('…');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_show() {
        let env = Envelope::new(Message::Show(Show {
            source: "backup".into(),
            title: "Veeam OK".into(),
            detail: None,
            priority: WirePriority::Info,
            ttl_ms: Some(5000),
            progress: None,
        }));
        let bytes = encode(&env);
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(decode_line(&bytes).unwrap(), env);
    }

    #[test]
    fn parses_handwritten_json() {
        let line = br#"{"v":1,"msg":{"type":"show","source":"x","title":"Ola"}}"#;
        let env = decode_line(line).unwrap();
        let Message::Show(s) = env.msg else { panic!() };
        assert_eq!(s.priority, WirePriority::Info);
        assert_eq!(s.ttl_ms, None);
    }

    #[test]
    fn absurd_ttl_is_capped() {
        let line = br#"{"v":1,"msg":{"type":"show","source":"x","title":"y","ttl_ms":18446744073709551615}}"#;
        let Message::Show(s) = decode_line(line).unwrap().msg else { panic!() };
        assert_eq!(s.ttl_ms, Some(MAX_TTL_MS));
    }

    #[test]
    fn accepts_utf8_bom_from_dotnet_clients() {
        let line = b"\xEF\xBB\xBF{\"v\":1,\"msg\":{\"type\":\"clear\",\"source\":\"x\"}}\r\n";
        assert_eq!(decode_line(line).unwrap().msg, Message::Clear { source: "x".into() });
    }

    #[test]
    fn rejects_attention_from_outside() {
        let line = br#"{"v":1,"msg":{"type":"show","source":"x","title":"y","priority":"attention"}}"#;
        assert!(matches!(decode_line(line), Err(ProtoError::Malformed(_))));
    }

    #[test]
    fn rejects_wrong_version_and_oversize() {
        let line = br#"{"v":99,"msg":{"type":"clear","source":"x"}}"#;
        assert_eq!(decode_line(line), Err(ProtoError::UnsupportedVersion(99)));
        let big = vec![b'a'; MAX_FRAME_BYTES + 1];
        assert!(matches!(decode_line(&big), Err(ProtoError::TooLarge(_))));
    }

    #[test]
    fn unknown_hook_event_is_other() {
        let line = br#"{"v":1,"msg":{"type":"claude_hook","event":"pre_compact","session_id":"s"}}"#;
        let Message::ClaudeHook(h) = decode_line(line).unwrap().msg else { panic!() };
        assert_eq!(h.event, HookEvent::Other);
    }

    #[test]
    fn sanitizes_text_and_progress() {
        let title = "á".repeat(500);
        let line = format!(
            r#"{{"v":1,"msg":{{"type":"show","source":"x","title":"{title}","progress":7.0}}}}"#
        );
        let Message::Show(s) = decode_line(line.as_bytes()).unwrap().msg else { panic!() };
        assert_eq!(s.title.chars().count(), MAX_TITLE_CHARS + 1); // + reticências
        assert_eq!(s.progress, Some(1.0));
    }
}
