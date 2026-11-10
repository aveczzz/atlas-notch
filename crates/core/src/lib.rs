//! Núcleo da island. Regra de ouro: **nada aqui conhece o Windows**.
//!
//! Fluxo de dados (unidirecional):
//!
//! ```text
//! fontes (threads/callbacks) --Event--> Engine --> módulos --> Arbiter --> View --> renderer
//!                                          |
//!                                          +--> Command --> plataforma (play/pause, volume, foco)
//! ```
//!
//! Tudo o que está neste crate corre na thread de UI, sem locks e sem alocações no
//! caminho quente quando nada muda.
#![forbid(unsafe_code)]

pub mod anim;
pub mod command;
pub mod engine;
pub mod event;
pub mod module;
pub mod view;

pub use command::Command;
pub use engine::{Engine, Presented};
pub use event::Event;
pub use module::{Module, ModuleId};
pub use view::{
    Action, ActionKind, ClaudeWidget, Compact, ControlId, Icon, MediaWidget, MixerWidget, Priority, Row, SessionLine, SessionState,
    ContextUse, Toast, TrayItem, TrayWidget, UsageBar, View, VolumeLine, Widget,
};
