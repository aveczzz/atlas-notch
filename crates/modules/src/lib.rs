//! Módulos built-in. Cada um é uma máquina de estados pura (sem Win32, sem threads).
//!
//! Para adicionar um módulo novo:
//! 1. Cria `src/<nome>.rs` com um struct que implementa `island_core::Module`.
//! 2. Testa-o aqui, com relógio falso (ver testes existentes).
//! 3. Regista-o em `default_modules()`.
//! 4. Se precisa de dados do sistema, cria a fonte em `island-platform`.
//!
//! Se a coisa vive fora da app (um script, outro programa), não precisas de módulo:
//! usa o protocolo IPC e o `ExternalModule` trata disso.
#![forbid(unsafe_code)]

pub mod claude;
pub mod external;
pub mod media;
pub mod mixer;
pub mod notifications;
pub mod system;
pub mod tray;

use island_core::Module;

pub fn default_modules() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(claude::ClaudeModule::default()),
        Box::new(media::MediaModule::default()),
        Box::new(mixer::MixerModule::default()),
        Box::new(notifications::NotificationsModule::default()),
        Box::new(system::SystemModule::default()),
        Box::new(external::ExternalModule::default()),
        Box::new(tray::TrayModule::default()),
    ]
}
