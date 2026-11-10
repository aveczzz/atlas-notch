use crate::command::Command;
use crate::event::Event;
use crate::view::{Action, View, Widget};
use std::time::Instant;

pub type ModuleId = &'static str;

/// Um módulo é uma máquina de estados pura: recebe eventos, expõe uma view.
///
/// Não é `Send` de propósito: vive sempre na thread de UI, por isso não precisa
/// de `Mutex` nem `Arc` no estado.
///
/// O tempo chega sempre por parâmetro (`now`) em vez de `Instant::now()` lá dentro.
/// Assim os testes controlam o relógio.
pub trait Module {
    fn id(&self) -> ModuleId;

    /// Processa um evento. Devolve `true` se o estado visível pode ter mudado.
    /// Ignorar eventos irrelevantes deve ser barato (um `match` e `false`).
    fn on_event(&mut self, event: &Event, now: Instant) -> bool;

    /// O utilizador mexeu num controlo deste módulo. Comandos para a plataforma
    /// vão para `out`. Devolve `true` se o estado visível mudou.
    fn on_action(&mut self, action: &Action, now: Instant, out: &mut Vec<Command>) -> bool;

    /// O que este módulo quer mostrar agora, se alguma coisa.
    fn view(&self, now: Instant) -> Option<View>;

    /// O bloco deste módulo no painel expandido. Por omissão, nenhum.
    fn widget(&self, _now: Instant) -> Option<Widget> {
        None
    }

    /// Próximo instante em que a view muda sozinha (ex.: um toast expira).
    /// O event loop só arma um timer se algum módulo devolver `Some`:
    /// é isto que garante zero wakeups em repouso.
    fn deadline(&self) -> Option<Instant> {
        None
    }

    /// Chamado quando o deadline passa, para limpar estado expirado.
    fn on_deadline(&mut self, _now: Instant) {}
}
