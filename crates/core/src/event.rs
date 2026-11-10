//! Eventos que entram no engine. Todos são `Send`: são produzidos em threads de
//! fontes (pipe, WinRT, Core Audio) e consumidos na thread de UI.

use crate::view::Action;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Media(MediaEvent),
    Audio(AudioEvent),
    Notification(NotificationEvent),
    /// Coisas do sistema: Caps Lock, bateria, dispositivo de áudio, copiar.
    System(SystemEvent),
    /// Ficheiros largados na island (separador Tray), já com o ícone.
    Tray(TrayEvent),
    /// Mensagem recebida pelo named pipe.
    External(island_proto::Message),
    /// O utilizador interagiu com um controlo da island.
    Action(Action),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MediaEvent {
    Changed(MediaState),
    /// Não há nenhuma sessão de media ativa.
    Cleared,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaState {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// AUMID da app (Spotify, Edge, ...).
    pub source_app: String,
    pub artwork: Option<ImageData>,
    /// Ícone da app que está a tocar (PNG), para o badge sobre a capa.
    pub app_icon: Option<ImageData>,
}

/// Bytes codificados (PNG/JPEG) de uma imagem. `Arc` para clonar sem copiar.
/// A descodificação para bitmap de GPU é feita pelo renderer e fica em cache.
#[derive(Clone, PartialEq, Eq)]
pub struct ImageData(pub Arc<[u8]>);

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageData({} bytes)", self.0.len())
    }
}

/// Identificador estável de uma sessão de áudio (o session instance identifier do Windows).
pub type AudioSessionId = String;

#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent {
    /// Lista completa de sessões (envia-se quando uma sessão aparece ou desaparece).
    Sessions(Vec<AudioSession>),
    SessionVolume { id: AudioSessionId, volume: f32, muted: bool },
    /// `quiet`: não mostrar o HUD. A fonte marca assim o estado inicial, a troca de
    /// dispositivo e os ecos dos nossos próprios comandos (pelo `event_context` do Core
    /// Audio, que é exato, em vez de adivinhar pelo valor).
    MasterVolume { volume: f32, muted: bool, quiet: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSession {
    pub id: AudioSessionId,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationEvent {
    /// Id do `UserNotification`, para poder ser retirada.
    pub id: u32,
    pub app: String,
    pub title: String,
    pub body: Option<String>,
    /// Ícone da app (PNG), do Shell.
    pub icon: Option<ImageData>,
    /// AppUserModelId, para abrir a app ao clicar no toast.
    pub aumid: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrayEvent {
    Added(Vec<TrayFile>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrayFile {
    /// Caminho completo (identifica o ficheiro na lista).
    pub path: String,
    /// Nome para mostrar (sem pasta).
    pub name: String,
    /// Ícone do Shell (PNG).
    pub icon: Option<ImageData>,
}

/// Teclas de bloqueio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKey {
    Caps,
    Num,
    Scroll,
}

/// O que foi copiado. Só o tipo: o conteúdo nunca sai do Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyKind {
    Text,
    Image,
    Files(u32),
}

/// Avisos curtos do sistema operativo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemEvent {
    /// Uma tecla de bloqueio mudou (`on` = o estado depois de premida).
    Lock { key: LockKey, on: bool },
    /// O carregador foi ligado ou desligado.
    Power { charging: bool, percent: u8 },
    /// A bateria desceu para um limiar (20% e 10%) sem carregador.
    BatteryLow { percent: u8 },
    /// O dispositivo de saída de áudio por omissão mudou.
    AudioDevice { name: String },
    /// Algo foi copiado.
    Copied(CopyKind),
}
