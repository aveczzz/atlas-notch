//! Estado das sessões do Claude Code, alimentado pelos hooks via `island-hook`.
//!
//! Suporta várias sessões em paralelo (vários projetos abertos no VS Code).
//! A island mostra a mais urgente; expandida, mostra todas.

use island_core::command::Command;
use island_core::event::Event;
use island_core::view::{
    Button, ClaudeWidget, Compact, ContextUse, ControlId, Icon, Priority, Row, SessionLine, SessionState, UsageBar, View,
    Widget,
};
use island_core::{Action, ActionKind, Module, ModuleId};
use island_proto::{Agent, ClaudeHook, ClaudeUsage, HookEvent, Message};
use std::time::{Duration, Instant};

pub const ID: ModuleId = "claude";
const FOCUS: &str = "focus";
/// Quanto tempo "terminou" fica visível.
const DONE_VISIBLE: Duration = Duration::from_secs(8);
/// Sem eventos durante isto, assumimos que a sessão morreu (VS Code fechado sem SessionEnd).
const STALE_WORKING: Duration = Duration::from_secs(15 * 60);
const STALE_WAITING: Duration = Duration::from_secs(30 * 60);
/// Proteção contra sessões infinitas (ex.: um bug a gerar session_ids novos).
const MAX_SESSIONS: usize = 16;

#[derive(Debug, Clone, PartialEq)]
enum State {
    Idle,
    Working { tool: Option<String> },
    NeedsInput { message: Option<String> },
    Done { at: Instant },
}

#[derive(Debug, Clone)]
struct Session {
    key: u32,
    id: String,
    project: Option<String>,
    agent: Agent,
    state: State,
    last_event: Instant,
    /// Quando a island viu a sessão pela primeira vez (ou o `SessionStart`).
    started: Instant,
    model: Option<String>,
    /// Tokens no contexto depois da última resposta.
    context_tokens: Option<u64>,
    /// O maior contexto visto: passou de 200 mil, a janela é de 1 milhão.
    max_context: u64,
}

impl Session {
    /// Urgência para escolher qual sessão aparece em modo compacto.
    fn urgency(&self, now: Instant) -> Option<Priority> {
        match &self.state {
            State::Idle => None,
            State::NeedsInput { .. } => Some(Priority::Attention),
            State::Working { .. } => Some(Priority::Active),
            State::Done { at } if now < *at + DONE_VISIBLE => Some(Priority::Info),
            State::Done { .. } => None,
        }
    }

    fn label(&self) -> &str {
        self.project.as_deref().unwrap_or("Claude")
    }

    fn status_text(&self) -> String {
        match &self.state {
            State::Idle => "inativo".into(),
            State::NeedsInput { .. } => "precisa de ti".into(),
            State::Working { tool: Some(t) } => tool_label(t),
            State::Working { tool: None } => "a pensar".into(),
            State::Done { .. } => "terminou".into(),
        }
    }

    fn expiry(&self) -> Option<Instant> {
        match self.state {
            State::Idle => None,
            State::Working { .. } => Some(self.last_event + STALE_WORKING),
            State::NeedsInput { .. } => Some(self.last_event + STALE_WAITING),
            State::Done { at } => Some(at + DONE_VISIBLE),
        }
    }
}

fn tool_label(tool: &str) -> String {
    match tool {
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => "a editar".into(),
        "Read" | "Grep" | "Glob" => "a ler código".into(),
        "Bash" | "PowerShell" => "a correr comandos".into(),
        "WebSearch" | "WebFetch" => "a pesquisar".into(),
        "Task" | "Agent" => "com subagentes".into(),
        // Ferramentas MCP chamam-se `mcp__<servidor>__<ferramenta>`: mostrar só o servidor.
        other => match other.strip_prefix("mcp__").and_then(|rest| rest.split("__").next()) {
            Some(server) => format!("a usar {server}"),
            None => other.to_owned(),
        },
    }
}

/// Ferramentas que param o Claude à espera de uma resposta tua.
fn asks_the_user(tool: Option<&str>) -> Option<&'static str> {
    match tool? {
        "AskUserQuestion" => Some("tem uma pergunta"),
        "ExitPlanMode" => Some("plano para aprovar"),
        _ => None,
    }
}

/// Uma janela de limite (5 horas ou semana).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Window {
    pct: f32,
    /// Quando reinicia. Depois disto a percentagem já não vale.
    resets_at: Option<Instant>,
}

impl Window {
    /// Ainda vale? A percentagem só deixa de valer quando a janela reinicia (não desce com o
    /// Claude Code fechado).
    fn live(&self, now: Instant) -> bool {
        self.resets_at.is_none_or(|t| t > now)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Usage {
    five_hour: Option<Window>,
    week: Option<Window>,
}

#[derive(Debug, Default)]
pub struct ClaudeModule {
    sessions: Vec<Session>,
    next_key: u32,
    usage: Usage,
}

/// Janela de contexto: 200 mil tokens, ou 1 milhão quando a conversa já passou dos 200 mil
/// (a versão de 1 milhão não se distingue pelo nome do modelo no ficheiro da conversa).
fn context_window(max_seen: u64) -> u64 {
    if max_seen > 200_000 { 1_000_000 } else { 200_000 }
}

/// "Opus 5.5" a partir de `claude-opus-5-5` (ou `claude-3-5-sonnet-20241022`).
fn short_model(id: &str) -> Option<String> {
    let parts: Vec<&str> = id.split('-').collect();
    let family = parts.iter().find(|p| matches!(**p, "opus" | "sonnet" | "haiku" | "fable"))?;
    // Os números da versão têm 1 ou 2 dígitos; a data (20241022) fica de fora.
    let version: Vec<&str> =
        parts.iter().filter(|p| !p.is_empty() && p.len() <= 2 && p.bytes().all(|b| b.is_ascii_digit())).copied().collect();
    let mut name = family[..1].to_ascii_uppercase() + &family[1..];
    if !version.is_empty() {
        name = format!("{name} {}", version.join("."));
    }
    Some(name)
}

/// "agora", "12 min", "1 h 05".
fn since(d: Duration) -> String {
    let mins = d.as_secs() / 60;
    match mins {
        0 => "agora".into(),
        1..=59 => format!("{mins} min"),
        _ => format!("{} h {:02}", mins / 60, mins % 60),
    }
}

/// Tempo até reiniciar, curto: "45m", "2h10", "3d4h".
fn remaining(d: Duration) -> String {
    let mins = d.as_secs().div_ceil(60);
    match mins {
        0..=59 => format!("{mins}m"),
        60..=1439 => format!("{}h{:02}", mins / 60, mins % 60),
        _ => format!("{}d{}h", mins / 1440, mins % 1440 / 60),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl ClaudeModule {
    fn apply(&mut self, h: &ClaudeHook, now: Instant) -> bool {
        if h.event == HookEvent::SessionEnd {
            let before = self.sessions.len();
            self.sessions.retain(|s| s.id != h.session_id);
            return self.sessions.len() != before;
        }
        let new_state = match h.event {
            HookEvent::SessionStart => State::Idle,
            HookEvent::UserPromptSubmit | HookEvent::PostToolUse | HookEvent::SubagentStop => {
                State::Working { tool: None }
            }
            HookEvent::PreToolUse => match asks_the_user(h.tool.as_deref()) {
                Some(why) => State::NeedsInput { message: Some(why.into()) },
                None => State::Working { tool: h.tool.clone() },
            },
            HookEvent::Notification => State::NeedsInput { message: h.message.clone() },
            HookEvent::Stop => State::Done { at: now },
            HookEvent::SessionEnd | HookEvent::Other => return false,
        };

        let session = match self.sessions.iter_mut().position(|s| s.id == h.session_id) {
            Some(i) => &mut self.sessions[i],
            None => {
                if self.sessions.len() >= MAX_SESSIONS {
                    // Descarta a sessão com atividade mais antiga.
                    if let Some(oldest) = self.sessions.iter().enumerate().min_by_key(|(_, s)| s.last_event) {
                        let i = oldest.0;
                        self.sessions.remove(i);
                    }
                }
                self.next_key += 1;
                self.sessions.push(Session {
                    key: self.next_key,
                    id: h.session_id.clone(),
                    project: None,
                    agent: h.agent,
                    state: State::Idle,
                    last_event: now,
                    started: now,
                    model: None,
                    context_tokens: None,
                    max_context: 0,
                });
                self.sessions.last_mut().expect("acabou de ser inserida")
            }
        };
        session.last_event = now;
        session.agent = h.agent;
        if h.event == HookEvent::SessionStart {
            session.started = now;
        }
        if h.project.is_some() {
            session.project = h.project.clone();
        }
        // O modelo e o contexto vêm do ficheiro da conversa, lido pelo hook.
        let mut changed = false;
        if h.model.is_some() && session.model != h.model {
            session.model.clone_from(&h.model);
            changed = true;
        }
        if let Some(tokens) = h.context_tokens
            && session.context_tokens != Some(tokens)
        {
            session.context_tokens = Some(tokens);
            session.max_context = session.max_context.max(tokens);
            changed = true;
        }
        // Durante uma pergunta pendente, PostToolUse de subagentes não deve esconder o pedido.
        if matches!(session.state, State::NeedsInput { .. })
            && matches!(h.event, HookEvent::SubagentStop)
        {
            return changed;
        }
        if session.state == new_state {
            return changed;
        }
        session.state = new_state;
        true
    }

    /// Limites da conta, vindos da linha de estado do Claude Code. `unix_now`: segundos desde
    /// 1970 agora (parâmetro para os testes não dependerem do relógio).
    fn apply_usage(&mut self, u: &ClaudeUsage, now: Instant, unix_now: u64) -> bool {
        let window = |w: Option<island_proto::UsageWindow>| {
            w.map(|w| Window {
                pct: w.used_pct,
                // Já passou: a janela reiniciou e a percentagem não vale (o `now` expira logo).
                resets_at: w.resets_at.map(|r| now + Duration::from_secs(r.saturating_sub(unix_now))),
            })
        };
        let next = Usage { five_hour: window(u.five_hour), week: window(u.seven_day) };
        if next == self.usage {
            return false;
        }
        self.usage = next;
        true
    }

    fn most_urgent(&self, now: Instant) -> Option<(&Session, Priority)> {
        self.sessions
            .iter()
            .filter_map(|s| s.urgency(now).map(|p| (s, p)))
            .max_by_key(|(s, p)| (*p, s.last_event))
    }
}

impl Module for ClaudeModule {
    fn id(&self) -> ModuleId {
        ID
    }

    fn on_event(&mut self, event: &Event, now: Instant) -> bool {
        match event {
            Event::External(Message::ClaudeHook(h)) => self.apply(h, now),
            Event::External(Message::ClaudeUsage(u)) => self.apply_usage(u, now, unix_now()),
            _ => false,
        }
    }

    fn on_action(&mut self, action: &Action, _now: Instant, out: &mut Vec<Command>) -> bool {
        if action.kind != ActionKind::Press || action.control.name != FOCUS {
            return false;
        }
        if let Some(s) = self.sessions.iter().find(|s| s.key == action.control.index) {
            out.push(Command::FocusEditor { project: s.project.clone() });
        }
        false
    }

    fn view(&self, now: Instant) -> Option<View> {
        let (top, priority) = self.most_urgent(now)?;
        let active = self.sessions.iter().filter(|s| s.urgency(now).is_some()).count();
        let mut text = format!("{}: {}", top.label(), top.status_text());
        if active > 1 {
            text.push_str(&format!(" +{}", active - 1));
        }

        let mut expanded = Vec::new();
        for s in self.sessions.iter().filter(|s| s.urgency(now).is_some()) {
            let secondary = match &s.state {
                State::NeedsInput { message } => message.clone(),
                _ => None,
            };
            expanded.push(Row::Text { primary: format!("{}: {}", s.label(), s.status_text()), secondary });
            expanded.push(Row::Buttons(vec![Button {
                control: ControlId::indexed(FOCUS, s.key),
                icon: None,
                label: Some("Abrir no VS Code".into()),
            }]));
        }

        Some(View {
            priority,
            compact: Compact {
                icon: match top.agent {
                    Agent::Claude => Icon::Claude,
                    Agent::Codex => Icon::Codex,
                },
                text,
                progress: None,
                pulse: priority == Priority::Attention,
                toast: None,
            },
            expanded,
        })
    }

    /// Sempre presente: sem sessões ativas o bloco diz "Sem sessões".
    fn widget(&self, now: Instant) -> Option<Widget> {
        let sessions = self
            .sessions
            .iter()
            .filter(|s| s.urgency(now).is_some())
            .filter_map(|s| {
                let state = match s.state {
                    State::Working { .. } => SessionState::Working,
                    State::NeedsInput { .. } => SessionState::NeedsInput,
                    State::Done { .. } => SessionState::Done,
                    State::Idle => return None,
                };
                let context = s.context_tokens.map(|tokens| {
                    let fraction = (tokens as f32 / context_window(s.max_context) as f32).clamp(0.0, 1.0);
                    ContextUse { fraction, label: format!("{:.0}%", fraction * 100.0) }
                });
                Some(SessionLine {
                    label: s.label().to_owned(),
                    // O que está a fazer, e há quanto tempo a sessão anda (ou terminou).
                    status: format!("{} · {}", s.status_text(), since(now.saturating_duration_since(s.started))),
                    state,
                    focus: ControlId::indexed(FOCUS, s.key),
                    model: s.model.as_deref().and_then(short_model),
                    context,
                })
            })
            .collect();
        let bar = |name: &str, w: Option<Window>| {
            let w = w.filter(|w| w.live(now))?;
            let fraction = (w.pct / 100.0).clamp(0.0, 1.0);
            let mut label = format!("{:.0}%", w.pct);
            if let Some(t) = w.resets_at {
                label.push_str(&format!(" · {}", remaining(t.saturating_duration_since(now))));
            }
            Some(UsageBar { name: name.to_owned(), fraction, label })
        };
        let usage = [bar("5 h", self.usage.five_hour), bar("Semana", self.usage.week)].into_iter().flatten().collect();
        Some(Widget::Claude(ClaudeWidget { sessions, usage }))
    }

    fn deadline(&self) -> Option<Instant> {
        self.sessions.iter().filter_map(Session::expiry).min()
    }

    fn on_deadline(&mut self, now: Instant) {
        for s in &mut self.sessions {
            if s.expiry().is_some_and(|t| t <= now) {
                s.state = State::Idle;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(event: HookEvent, session: &str) -> Event {
        Event::External(Message::ClaudeHook(ClaudeHook {
            agent: Agent::Claude,
            event,
            session_id: session.into(),
            project: Some(format!("proj-{session}")),
            tool: (event == HookEvent::PreToolUse).then(|| "Edit".into()),
            message: (event == HookEvent::Notification).then(|| "Permitir Bash?".into()),
            model: None,
            context_tokens: None,
        }))
    }

    #[test]
    fn lifecycle() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        m.on_event(&hook(HookEvent::SessionStart, "a"), t0);
        assert!(m.view(t0).is_none(), "sessão inativa não aparece");

        m.on_event(&hook(HookEvent::PreToolUse, "a"), t0);
        let v = m.view(t0).unwrap();
        assert_eq!(v.priority, Priority::Active);
        assert_eq!(v.compact.text, "proj-a: a editar");

        m.on_event(&hook(HookEvent::Notification, "a"), t0);
        let v = m.view(t0).unwrap();
        assert_eq!(v.priority, Priority::Attention);
        assert!(v.compact.pulse);

        m.on_event(&hook(HookEvent::Stop, "a"), t0);
        assert_eq!(m.view(t0).unwrap().priority, Priority::Info);

        let d = m.deadline().unwrap();
        assert_eq!(d, t0 + DONE_VISIBLE);
        m.on_deadline(d);
        assert!(m.view(d).is_none());
        assert!(m.deadline().is_none(), "nada pendente = sem timers");
    }

    #[test]
    fn most_urgent_session_wins_and_others_are_counted() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        m.on_event(&hook(HookEvent::PreToolUse, "a"), t0);
        m.on_event(&hook(HookEvent::Notification, "b"), t0);
        let v = m.view(t0).unwrap();
        assert_eq!(v.compact.text, "proj-b: precisa de ti +1");
    }

    #[test]
    fn focus_button_targets_right_project() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        m.on_event(&hook(HookEvent::PreToolUse, "a"), t0);
        m.on_event(&hook(HookEvent::PreToolUse, "b"), t0);
        let key_b = m.sessions[1].key;
        m.on_event(&hook(HookEvent::SessionEnd, "a"), t0);
        let mut out = vec![];
        let action = Action { module: ID, control: ControlId::indexed(FOCUS, key_b), kind: ActionKind::Press };
        m.on_action(&action, t0, &mut out);
        assert_eq!(out, vec![Command::FocusEditor { project: Some("proj-b".into()) }]);
    }

    #[test]
    fn widget_lists_active_sessions_with_their_state() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        let Some(Widget::Claude(w)) = m.widget(t0) else { panic!() };
        assert!(w.sessions.is_empty(), "sem sessões o bloco existe e está vazio");

        m.on_event(&hook(HookEvent::SessionStart, "idle"), t0);
        m.on_event(&hook(HookEvent::PreToolUse, "a"), t0);
        m.on_event(&hook(HookEvent::Notification, "b"), t0);
        let Some(Widget::Claude(w)) = m.widget(t0) else { panic!() };
        let states: Vec<_> = w.sessions.iter().map(|s| (s.label.as_str(), s.state)).collect();
        assert_eq!(states, vec![("proj-a", SessionState::Working), ("proj-b", SessionState::NeedsInput)]);

        // O botão de cada linha abre o projeto certo.
        let mut out = vec![];
        let action = Action { module: ID, control: w.sessions[1].focus, kind: ActionKind::Press };
        m.on_action(&action, t0, &mut out);
        assert_eq!(out, vec![Command::FocusEditor { project: Some("proj-b".into()) }]);
    }

    #[test]
    fn questions_and_plans_need_the_user() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        let mut ask = hook(HookEvent::PreToolUse, "a");
        if let Event::External(Message::ClaudeHook(h)) = &mut ask {
            h.tool = Some("AskUserQuestion".into());
        }
        m.on_event(&ask, t0);
        let v = m.view(t0).unwrap();
        assert_eq!(v.priority, Priority::Attention);
        assert!(v.compact.pulse);
        // A resposta chega como PostToolUse: volta a trabalhar.
        m.on_event(&hook(HookEvent::PostToolUse, "a"), t0);
        assert_eq!(m.view(t0).unwrap().priority, Priority::Active);
    }

    #[test]
    fn mcp_tools_show_the_server_name() {
        assert_eq!(tool_label("mcp__claude-vscode__getDiagnostics"), "a usar claude-vscode");
        assert_eq!(tool_label("Bash"), "a correr comandos");
        assert_eq!(tool_label("Outra"), "Outra");
    }

    #[test]
    fn session_count_is_bounded() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        for i in 0..100u64 {
            m.on_event(&hook(HookEvent::PreToolUse, &i.to_string()), t0 + Duration::from_millis(i));
        }
        assert_eq!(m.sessions.len(), MAX_SESSIONS);
    }

    #[test]
    fn stale_working_session_disappears() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        m.on_event(&hook(HookEvent::PreToolUse, "a"), t0);
        m.on_deadline(t0 + STALE_WORKING);
        assert!(m.view(t0 + STALE_WORKING).is_none());
    }

    fn claude_widget(m: &ClaudeModule, now: Instant) -> ClaudeWidget {
        let Some(Widget::Claude(w)) = m.widget(now) else { panic!("sem widget") };
        w
    }

    fn hook_with(event: HookEvent, session: &str, model: Option<&str>, tokens: Option<u64>) -> Event {
        let Event::External(Message::ClaudeHook(mut h)) = hook(event, session) else { unreachable!() };
        h.model = model.map(str::to_owned);
        h.context_tokens = tokens;
        Event::External(Message::ClaudeHook(h))
    }

    #[test]
    fn session_line_shows_duration_model_and_context() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        m.on_event(&hook_with(HookEvent::PreToolUse, "a", Some("claude-opus-5-5"), Some(50_000)), t0);
        let t1 = t0 + Duration::from_secs(12 * 60 + 5);
        let line = claude_widget(&m, t1).sessions.remove(0);
        assert_eq!(line.status, "a editar · 12 min");
        assert_eq!(line.model.as_deref(), Some("Opus 5.5"));
        let ctx = line.context.unwrap();
        assert_eq!(ctx.label, "25%", "50 mil de 200 mil");
        assert!((ctx.fraction - 0.25).abs() < 1e-6);
    }

    #[test]
    fn context_window_grows_to_a_million_once_it_passes_200k() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        m.on_event(&hook_with(HookEvent::PreToolUse, "a", None, Some(431_000)), t0);
        assert_eq!(claude_widget(&m, t0).sessions[0].context.as_ref().unwrap().label, "43%");
        // Depois de passar os 200 mil continua a contar com 1 milhão, mesmo que desça.
        m.on_event(&hook_with(HookEvent::PostToolUse, "a", None, Some(150_000)), t0);
        assert_eq!(claude_widget(&m, t0).sessions[0].context.as_ref().unwrap().label, "15%");
    }

    #[test]
    fn model_or_context_change_alone_redraws() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        m.on_event(&hook_with(HookEvent::PreToolUse, "a", None, Some(10)), t0);
        // Mesmo estado ("a editar"), mas o contexto subiu: é uma mudança.
        assert!(m.on_event(&hook_with(HookEvent::PreToolUse, "a", None, Some(20)), t0));
        assert!(!m.on_event(&hook_with(HookEvent::PreToolUse, "a", None, Some(20)), t0));
    }

    #[test]
    fn model_names_are_short() {
        assert_eq!(short_model("claude-opus-5-5").as_deref(), Some("Opus 5.5"));
        assert_eq!(short_model("claude-sonnet-5-5").as_deref(), Some("Sonnet 5.5"));
        assert_eq!(short_model("claude-3-5-sonnet-20241022").as_deref(), Some("Sonnet 3.5"));
        assert_eq!(short_model("claude-haiku-4-5-20251001").as_deref(), Some("Haiku 4.5"));
        assert_eq!(short_model("<synthetic>"), None);
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(since(Duration::from_secs(20)), "agora");
        assert_eq!(since(Duration::from_secs(59 * 60)), "59 min");
        assert_eq!(since(Duration::from_secs(65 * 60)), "1 h 05");
        assert_eq!(remaining(Duration::from_secs(45 * 60)), "45m");
        assert_eq!(remaining(Duration::from_secs(130 * 60)), "2h10");
        assert_eq!(remaining(Duration::from_secs(3 * 86_400 + 4 * 3600)), "3d4h");
    }

    #[test]
    fn account_limits_show_until_the_window_resets() {
        let t0 = Instant::now();
        let unix = 1_000_000;
        let mut m = ClaudeModule::default();
        let usage = ClaudeUsage {
            five_hour: Some(island_proto::UsageWindow { used_pct: 37.4, resets_at: Some(unix + 130 * 60) }),
            seven_day: Some(island_proto::UsageWindow { used_pct: 12.0, resets_at: Some(unix + 3 * 86_400) }),
        };
        assert!(m.apply_usage(&usage, t0, unix));
        assert!(!m.apply_usage(&usage, t0, unix), "o mesmo valor não redesenha");
        let bars = claude_widget(&m, t0).usage;
        assert_eq!((bars[0].name.as_str(), bars[0].label.as_str()), ("5 h", "37% · 2h10"));
        assert_eq!((bars[1].name.as_str(), bars[1].label.as_str()), ("Semana", "12% · 3d0h"));
        // Duas horas e dez depois a janela de 5 h reiniciou: a barra sai; a semana fica.
        let later = t0 + Duration::from_secs(131 * 60);
        let bars = claude_widget(&m, later).usage;
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].name, "Semana");
    }

    #[test]
    fn a_missing_window_is_just_left_out() {
        let t0 = Instant::now();
        let mut m = ClaudeModule::default();
        let only_week = ClaudeUsage { five_hour: None, seven_day: Some(island_proto::UsageWindow { used_pct: 80.0, resets_at: None }) };
        m.apply_usage(&only_week, t0, 0);
        let bars = claude_widget(&m, t0).usage;
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].label, "80%");
    }
}
