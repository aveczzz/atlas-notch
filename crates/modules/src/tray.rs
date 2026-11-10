//! Tray: ficheiros largados na island para ficarem à mão (separador "Tray").
//!
//! Só guarda a lista: o arrastar para dentro e para fora, os ícones e abrir os ficheiros
//! são da plataforma. A lista é limitada (regra 6): ao passar do limite sai o mais antigo.
//! Não toca nos ficheiros: tirar da lista não apaga nada.

use island_core::command::Command;
use island_core::event::{Event, TrayEvent, TrayFile};
use island_core::view::{ControlId, TrayItem, TrayWidget, View, Widget};
use island_core::{Action, ActionKind, Module, ModuleId};
use std::time::Instant;

pub const ID: ModuleId = "tray";
const OPEN: &str = "open";
const REMOVE: &str = "remove";
/// Cabem 7 numa fila do painel.
pub const MAX_FILES: usize = 7;

#[derive(Debug, Default)]
pub struct TrayModule {
    /// Mais recente primeiro. A chave estável vai no `ControlId::index`, como no misturador:
    /// um clique nunca acerta no ficheiro errado se a lista mudar entretanto.
    files: Vec<(u32, TrayFile)>,
    next_key: u32,
}

impl Module for TrayModule {
    fn id(&self) -> ModuleId {
        ID
    }

    fn on_event(&mut self, event: &Event, _now: Instant) -> bool {
        let Event::Tray(TrayEvent::Added(added)) = event else { return false };
        for file in added.iter().rev() {
            // O mesmo ficheiro outra vez sobe para o início em vez de duplicar.
            self.files.retain(|(_, f)| f.path != file.path);
            self.next_key += 1;
            self.files.insert(0, (self.next_key, file.clone()));
        }
        self.files.truncate(MAX_FILES);
        !added.is_empty()
    }

    fn on_action(&mut self, action: &Action, _now: Instant, out: &mut Vec<Command>) -> bool {
        if action.kind != ActionKind::Press {
            return false;
        }
        let Some(i) = self.files.iter().position(|(k, _)| *k == action.control.index) else { return false };
        match action.control.name {
            OPEN => {
                out.push(Command::OpenFile { path: self.files[i].1.path.clone() });
                false
            }
            REMOVE => {
                self.files.remove(i);
                true
            }
            _ => false,
        }
    }

    /// Não disputa o notch compacto: só existe no painel.
    fn view(&self, _now: Instant) -> Option<View> {
        None
    }

    fn widget(&self, _now: Instant) -> Option<Widget> {
        let items = self
            .files
            .iter()
            .map(|(k, f)| TrayItem {
                path: f.path.clone(),
                name: f.name.clone(),
                icon: f.icon.clone(),
                open: ControlId::indexed(OPEN, *k),
                remove: ControlId::indexed(REMOVE, *k),
            })
            .collect();
        Some(Widget::Tray(TrayWidget { items }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> TrayFile {
        TrayFile { path: format!(r"C:\x\{name}"), name: name.into(), icon: None }
    }

    fn add(m: &mut TrayModule, names: &[&str]) -> bool {
        m.on_event(&Event::Tray(TrayEvent::Added(names.iter().map(|n| file(n)).collect())), Instant::now())
    }

    fn names(m: &TrayModule) -> Vec<String> {
        let Some(Widget::Tray(w)) = m.widget(Instant::now()) else { panic!() };
        w.items.into_iter().map(|i| i.name).collect()
    }

    #[test]
    fn newest_first_without_duplicates_and_bounded() {
        let mut m = TrayModule::default();
        assert!(names(&m).is_empty(), "o separador existe mesmo vazio");
        assert!(add(&mut m, &["a.pdf", "b.png"]));
        assert_eq!(names(&m), ["a.pdf", "b.png"], "a ordem do arrastar mantém-se");
        add(&mut m, &["a.pdf"]);
        assert_eq!(names(&m), ["a.pdf", "b.png"], "repetido sobe, não duplica");
        add(&mut m, &["c", "d", "e", "f", "g", "h", "i"]);
        assert_eq!(names(&m).len(), MAX_FILES);
        assert_eq!(names(&m)[0], "c");
    }

    #[test]
    fn open_and_remove_use_stable_keys() {
        let mut m = TrayModule::default();
        add(&mut m, &["a.pdf", "b.png"]);
        let Some(Widget::Tray(w)) = m.widget(Instant::now()) else { panic!() };
        let (open_b, remove_a) = (w.items[1].open, w.items[0].remove);

        let mut out = vec![];
        assert!(m.on_action(&Action { module: ID, control: remove_a, kind: ActionKind::Press }, Instant::now(), &mut out));
        assert_eq!(names(&m), ["b.png"]);
        // A lista mudou, mas o "abrir" do b.png continua a apontar para ele.
        m.on_action(&Action { module: ID, control: open_b, kind: ActionKind::Press }, Instant::now(), &mut out);
        assert_eq!(out, vec![Command::OpenFile { path: r"C:\x\b.png".into() }]);
    }

    #[test]
    fn never_competes_for_the_compact_notch() {
        let mut m = TrayModule::default();
        add(&mut m, &["a.pdf"]);
        assert!(m.view(Instant::now()).is_none());
    }
}
