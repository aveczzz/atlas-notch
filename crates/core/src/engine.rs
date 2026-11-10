//! O engine junta os módulos e decide o que a island mostra (o árbitro).
//!
//! Regra do árbitro: ganha a maior `Priority`; em empate, o módulo cuja view mudou
//! mais recentemente. Simples, previsível, e fácil de explicar quando algo aparece
//! "no sítio errado".

use crate::command::Command;
use crate::event::Event;
use crate::module::{Module, ModuleId};
use crate::view::{View, Widget};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub struct Presented {
    pub module: ModuleId,
    pub view: View,
}

pub struct Engine {
    modules: Vec<Box<dyn Module>>,
    /// Instante da última mudança de cada módulo (desempate do árbitro).
    changed_at: Vec<Instant>,
    current: Option<Presented>,
    /// Blocos do painel expandido, pela ordem de registo dos módulos.
    panel: Vec<(ModuleId, Widget)>,
    commands: Vec<Command>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<_> = self.modules.iter().map(|m| m.id()).collect();
        f.debug_struct("Engine")
            .field("modules", &ids)
            .field("current", &self.current.as_ref().map(|p| p.module))
            .finish()
    }
}

impl Engine {
    pub fn new(modules: Vec<Box<dyn Module>>, now: Instant) -> Self {
        let n = modules.len();
        let mut engine = Self {
            modules,
            changed_at: vec![now; n],
            current: None,
            panel: Vec::new(),
            commands: Vec::new(),
        };
        engine.recompute(now);
        engine
    }

    /// Entrega um evento. Devolve `true` se a island precisa de ser redesenhada.
    pub fn dispatch(&mut self, event: Event, now: Instant) -> bool {
        let mut any = false;
        if let Event::Action(action) = &event {
            // Ações vão só para o módulo dono do controlo.
            if let Some(i) = self.modules.iter().position(|m| m.id() == action.module)
                && self.modules[i].on_action(action, now, &mut self.commands)
            {
                self.changed_at[i] = now;
                any = true;
            }
        } else {
            // Broadcast: com meia dúzia de módulos é mais barato do que manter um índice.
            for (i, m) in self.modules.iter_mut().enumerate() {
                if m.on_event(&event, now) {
                    self.changed_at[i] = now;
                    any = true;
                }
            }
        }
        any && self.recompute(now)
    }

    /// Chamar quando `next_deadline()` passa.
    pub fn on_deadline(&mut self, now: Instant) -> bool {
        for (i, m) in self.modules.iter_mut().enumerate() {
            if m.deadline().is_some_and(|d| d <= now) {
                m.on_deadline(now);
                self.changed_at[i] = now;
            }
        }
        self.recompute(now)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.modules.iter().filter_map(|m| m.deadline()).min()
    }

    pub fn current(&self) -> Option<&Presented> {
        self.current.as_ref()
    }

    pub fn panel(&self) -> &[(ModuleId, Widget)] {
        &self.panel
    }

    /// Comandos pendentes para a plataforma executar.
    pub fn drain_commands(&mut self) -> std::vec::Drain<'_, Command> {
        self.commands.drain(..)
    }

    /// Recalcula o vencedor e o painel. Devolve `true` só se algum deles mudou: o painel
    /// pode mudar sem a view vencedora mudar (ex.: a música troca enquanto o Claude ganha).
    fn recompute(&mut self, now: Instant) -> bool {
        let panel: Vec<_> = self.modules.iter().filter_map(|m| m.widget(now).map(|w| (m.id(), w))).collect();
        let panel_changed = panel != self.panel;
        if panel_changed {
            self.panel = panel;
        }
        self.recompute_current(now) | panel_changed
    }

    fn recompute_current(&mut self, now: Instant) -> bool {
        let mut best: Option<(usize, View)> = None;
        for (i, m) in self.modules.iter().enumerate() {
            let Some(v) = m.view(now) else { continue };
            let better = match &best {
                None => true,
                Some((j, bv)) => {
                    (v.priority, self.changed_at[i]) > (bv.priority, self.changed_at[*j])
                }
            };
            if better {
                best = Some((i, v));
            }
        }
        let next = best.map(|(i, view)| Presented { module: self.modules[i].id(), view });
        if next == self.current {
            return false;
        }
        self.current = next;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Action, ActionKind, Compact, ControlId, Icon, Priority};
    use std::time::Duration;

    /// Módulo de teste: mostra o texto do último `Clear{source}` com uma prioridade fixa.
    struct Fake {
        id: ModuleId,
        prio: Priority,
        text: Option<String>,
        until: Option<Instant>,
        /// Se tem bloco no painel (mostra o mesmo texto).
        widget: bool,
    }

    impl Fake {
        fn new(id: ModuleId, prio: Priority) -> Box<Self> {
            Box::new(Self { id, prio, text: None, until: None, widget: false })
        }
        fn with_widget(id: ModuleId, prio: Priority) -> Box<Self> {
            Box::new(Self { widget: true, ..*Self::new(id, prio) })
        }
    }

    impl Module for Fake {
        fn id(&self) -> ModuleId {
            self.id
        }
        fn on_event(&mut self, e: &Event, now: Instant) -> bool {
            match e {
                Event::External(island_proto::Message::Clear { source }) if source.starts_with(self.id) => {
                    self.text = Some(source.clone());
                    self.until = Some(now + Duration::from_secs(1));
                    true
                }
                _ => false,
            }
        }
        fn on_action(&mut self, _: &Action, _: Instant, out: &mut Vec<Command>) -> bool {
            out.push(Command::MediaPlayPause);
            false
        }
        fn view(&self, _: Instant) -> Option<View> {
            self.text.as_ref().map(|t| View {
                priority: self.prio,
                compact: Compact { icon: Icon::Bell, text: t.clone(), progress: None, pulse: false, toast: None },
                expanded: vec![],
            })
        }
        fn widget(&self, _: Instant) -> Option<Widget> {
            let text = self.text.clone().filter(|_| self.widget)?;
            Some(Widget::Rows(vec![crate::view::Row::Text { primary: text, secondary: None }]))
        }
        fn deadline(&self) -> Option<Instant> {
            self.until
        }
        fn on_deadline(&mut self, _: Instant) {
            self.text = None;
            self.until = None;
        }
    }

    fn ev(s: &str) -> Event {
        Event::External(island_proto::Message::Clear { source: s.into() })
    }

    #[test]
    fn highest_priority_wins_then_most_recent() {
        let t0 = Instant::now();
        let mut e = Engine::new(
            vec![Fake::new("a", Priority::Info), Fake::new("b", Priority::Active), Fake::new("c", Priority::Info)],
            t0,
        );
        assert!(e.current().is_none());

        assert!(e.dispatch(ev("a1"), t0));
        assert_eq!(e.current().unwrap().module, "a");

        assert!(e.dispatch(ev("c1"), t0 + Duration::from_millis(10)));
        assert_eq!(e.current().unwrap().module, "c", "empate: ganha o mais recente");

        assert!(e.dispatch(ev("b1"), t0 + Duration::from_millis(20)));
        assert_eq!(e.current().unwrap().module, "b", "prioridade maior ganha");

        // Atualizar "a" não muda nada visível: não pede redraw.
        assert!(!e.dispatch(ev("a2"), t0 + Duration::from_millis(30)));
    }

    #[test]
    fn panel_change_redraws_even_when_winner_stays() {
        let t0 = Instant::now();
        let mut e = Engine::new(vec![Fake::new("a", Priority::Active), Fake::with_widget("b", Priority::Info)], t0);
        e.dispatch(ev("a1"), t0);
        assert!(e.dispatch(ev("b1"), t0), "o bloco de b apareceu no painel");
        assert_eq!(e.current().unwrap().module, "a", "a continua a ganhar o compacto");
        assert!(e.dispatch(ev("b2"), t0), "o bloco de b mudou");
        assert_eq!(e.panel().len(), 1);
        assert_eq!(e.panel()[0].0, "b");
        assert!(!e.dispatch(ev("b2"), t0), "igual: sem redraw");
    }

    #[test]
    fn irrelevant_events_do_not_redraw() {
        let t0 = Instant::now();
        let mut e = Engine::new(vec![Fake::new("a", Priority::Info)], t0);
        assert!(!e.dispatch(ev("zzz"), t0));
    }

    #[test]
    fn deadlines_expire_views() {
        let t0 = Instant::now();
        let mut e = Engine::new(vec![Fake::new("a", Priority::Info)], t0);
        e.dispatch(ev("a1"), t0);
        let d = e.next_deadline().unwrap();
        assert_eq!(d, t0 + Duration::from_secs(1));
        assert!(e.on_deadline(d));
        assert!(e.current().is_none());
        assert!(e.next_deadline().is_none(), "sem deadlines = sem timers");
    }

    #[test]
    fn actions_route_only_to_owner() {
        let t0 = Instant::now();
        let mut e = Engine::new(vec![Fake::new("a", Priority::Info), Fake::new("b", Priority::Info)], t0);
        let action = Action { module: "b", control: ControlId::new("x"), kind: ActionKind::Press };
        e.dispatch(Event::Action(action), t0);
        assert_eq!(e.drain_commands().collect::<Vec<_>>(), vec![Command::MediaPlayPause]);
        assert_eq!(e.drain_commands().count(), 0);
    }
}
