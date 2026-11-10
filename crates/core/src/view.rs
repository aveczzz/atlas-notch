//! Modelo de apresentação. Os módulos devolvem uma `View` declarativa; o renderer
//! decide como a desenhar. `PartialEq` em tudo: o engine só pede redraw quando a
//! view vencedora muda de facto.

use crate::event::ImageData;
use crate::module::ModuleId;

/// Ordem importa: a derivação de `Ord` segue a ordem das variantes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Só aparece quando não há mais nada (ex.: misturador em repouso).
    Ambient,
    /// Informação passageira (notificação, "Claude terminou").
    Info,
    /// Atividade em curso (música a tocar, Claude a trabalhar).
    Active,
    /// Precisa de ti agora (Claude à espera de aprovação, mudança de volume).
    Attention,
}

impl From<island_proto::WirePriority> for Priority {
    fn from(p: island_proto::WirePriority) -> Self {
        match p {
            island_proto::WirePriority::Ambient => Self::Ambient,
            island_proto::WirePriority::Info => Self::Info,
            island_proto::WirePriority::Active => Self::Active,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub priority: Priority,
    pub compact: Compact,
    /// Linhas de detalhe do módulo (usadas pelo backend headless). O painel expandido
    /// da janela usa os `Widget`s de todos os módulos, não isto.
    pub expanded: Vec<Row>,
}

/// Bloco do painel expandido. Ao contrário da `View` (só a do módulo vencedor), o painel
/// mostra os blocos de **todos** os módulos lado a lado. Tipado para o renderer dar a cada
/// um o seu desenho (capa, controlos, pontos de estado); `Rows` serve o resto.
#[derive(Debug, Clone, PartialEq)]
pub enum Widget {
    Media(MediaWidget),
    Claude(ClaudeWidget),
    /// Volume geral e por app (separador "Som").
    Mixer(MixerWidget),
    /// Ficheiros guardados (separador "Tray").
    Tray(TrayWidget),
    /// Apps externas e módulos sem desenho próprio.
    Rows(Vec<Row>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaWidget {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub artwork: Option<ImageData>,
    /// Ícone da app que está a tocar (PNG), para o badge sobre a capa.
    pub app_icon: Option<ImageData>,
    pub prev: ControlId,
    pub play_pause: ControlId,
    pub next: ControlId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixerWidget {
    /// A primeira é o volume geral; as outras, uma por app.
    pub lines: Vec<VolumeLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VolumeLine {
    pub label: String,
    /// 0.0 a 1.0.
    pub value: f32,
    pub muted: bool,
    /// Slider: recebe `ActionKind::SetValue`.
    pub slider: ControlId,
    /// Botão de silenciar (`Press`), se a linha o tiver.
    pub mute: Option<ControlId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeWidget {
    /// Só as sessões com algo a mostrar. Vazio = "Sem sessões".
    pub sessions: Vec<SessionLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionLine {
    pub label: String,
    pub status: String,
    pub state: SessionState,
    /// Botão "Abrir no VS Code".
    pub focus: ControlId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Working,
    NeedsInput,
    Done,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Compact {
    pub icon: Icon,
    /// Vazio = só o ícone (a island fica na largura mínima).
    pub text: String,
    pub progress: Option<f32>,
    /// Pulsar suavemente (ex.: Claude à espera). O renderer só anima enquanto isto for true.
    pub pulse: bool,
    /// Notificação de uma app: o renderer mostra o toast grande (ícone, título e uma linha).
    pub toast: Option<Toast>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Icon {
    Music,
    Volume,
    Muted,
    Claude,
    Mail,
    Bell,
    Keyboard,
    Battery,
    BatteryCharging,
    Headphones,
    Copy,
    /// Ícone de uma app externa, por nome (resolvido pelo renderer).
    Named(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Text { primary: String, secondary: Option<String> },
    Slider { control: ControlId, label: String, value: f32 },
    Buttons(Vec<Button>),
    Progress(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub control: ControlId,
    pub icon: Option<Icon>,
    pub label: Option<String>,
}

/// Identifica um controlo dentro de um módulo. `index` serve para listas
/// (ex.: o slider da sessão n.º 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlId {
    pub name: &'static str,
    pub index: u32,
}

impl ControlId {
    pub const fn new(name: &'static str) -> Self {
        Self { name, index: 0 }
    }
    pub const fn indexed(name: &'static str, index: u32) -> Self {
        Self { name, index }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub module: ModuleId,
    pub control: ControlId,
    pub kind: ActionKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActionKind {
    Press,
    SetValue(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrayWidget {
    /// Mais recente primeiro. Vazio = "arrasta ficheiros para aqui".
    pub items: Vec<TrayItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrayItem {
    pub path: String,
    pub name: String,
    pub icon: Option<ImageData>,
    /// Clicar: abre o ficheiro.
    pub open: ControlId,
    /// O ✕: tira-o da lista (o ficheiro fica onde está).
    pub remove: ControlId,
}

/// Notificação de uma app em formato de toast. O título é o `Compact::text`.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    /// Nome da app ("Outlook").
    pub app: String,
    /// Uma linha de texto (pode estar vazia).
    pub body: String,
    /// Ícone da app (PNG).
    pub icon: Option<ImageData>,
    /// Clicar no toast: abre a app.
    pub open: ControlId,
}
