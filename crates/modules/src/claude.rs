//! Estado das sessões do Claude Code, alimentado pelos hooks via `island-hook`.
//!
//! Suporta várias sessões em paralelo (vários projetos abertos no VS Code).
//! A island mostra a mais urgente; expandida, mostra todas.

use island_core::command::Command;
use island_core::event::Event;
use island_core::view::{
    Button, ClaudeWidget, Compact, ControlId, Icon, Priority, Row, SessionLine, SessionState, View, Widget,
};
use island_core::{Action, ActionKind, Module, ModuleId};
use island_proto::{ClaudeHook, HookEvent, Message};
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
    state: State,
    last_event: Instant,
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

#[derive(Debug, Default)]
pub struct ClaudeModule {
    sessions: Vec<Session>,
    next_key: u32,
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
                    state: State::Idle,
                    last_event: now,
                });
                self.sessions.last_mut().expect("acabou de ser inserida")
            }
        };
        session.last_event = now;
        if h.project.is_some() {
            session.project = h.project.clone();
        }
        // Durante uma pergunta pendente, PostToolUse de subagentes não deve esconder o pedido.
        if matches!(session.state, State::NeedsInput { .. })
            && matches!(h.event, HookEvent::SubagentStop)
        {
            return false;
        }
        if session.state == new_state {
            return false;
        }
        session.state = new_state;
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
                icon: Icon::Claude,
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
                Some(SessionLine {
                    label: s.label().to_owned(),
                    status: s.status_text(),
                    state,
                    focus: ControlId::indexed(FOCUS, s.key),
                })
            })
            .collect();
        Some(Widget::Claude(ClaudeWidget { sessions }))
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
}
