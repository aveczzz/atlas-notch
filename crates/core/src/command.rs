//! Pedidos dos módulos à plataforma. Os módulos nunca chamam APIs do sistema
//! diretamente: descrevem o que querem e a plataforma executa. É isto que os
//! mantém testáveis.

use crate::event::AudioSessionId;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    MediaPlayPause,
    MediaNext,
    MediaPrevious,
    SetMasterVolume(f32),
    SetSessionVolume { id: AudioSessionId, volume: f32 },
    SetSessionMute { id: AudioSessionId, muted: bool },
    /// Traz para a frente a janela do VS Code deste projeto.
    FocusEditor { project: Option<String> },
    DismissNotification { id: u32 },
    /// Abrir um ficheiro com a app predefinida (Tray).
    OpenFile { path: String },
    /// Abrir uma app pelo AppUserModelId (clicar numa notificação).
    OpenApp { aumid: String },
}
