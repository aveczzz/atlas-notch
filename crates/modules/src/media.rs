//! Música e media (Spotify, browser, ...). Dados vêm do GSMTC na plataforma.

use island_core::command::Command;
use island_core::event::{Event, MediaEvent, MediaState};
use island_core::view::{Button, Compact, ControlId, Icon, MediaWidget, Priority, Row, View, Widget};
use island_core::{Action, ActionKind, Module, ModuleId};
use std::time::Instant;

pub const ID: ModuleId = "media";
const PREV: ControlId = ControlId::new("prev");
const PLAY: ControlId = ControlId::new("play");
const NEXT: ControlId = ControlId::new("next");

#[derive(Debug, Default)]
pub struct MediaModule {
    state: Option<MediaState>,
}

impl Module for MediaModule {
    fn id(&self) -> ModuleId {
        ID
    }

    fn on_event(&mut self, event: &Event, _now: Instant) -> bool {
        let Event::Media(e) = event else { return false };
        let next = match e {
            MediaEvent::Changed(s) => Some(s.clone()),
            MediaEvent::Cleared => None,
        };
        if next == self.state {
            return false; // o GSMTC repete eventos com frequência; ignorar duplicados
        }
        self.state = next;
        true
    }

    fn on_action(&mut self, action: &Action, _now: Instant, out: &mut Vec<Command>) -> bool {
        if action.kind != ActionKind::Press {
            return false;
        }
        let cmd = match action.control {
            PREV => Command::MediaPrevious,
            PLAY => Command::MediaPlayPause,
            NEXT => Command::MediaNext,
            _ => return false,
        };
        out.push(cmd);
        // Não alteramos `playing` de forma otimista: o GSMTC confirma em milissegundos
        // e assim nunca mostramos um estado errado.
        false
    }

    fn view(&self, _now: Instant) -> Option<View> {
        let s = self.state.as_ref()?;
        let priority = if s.playing { Priority::Active } else { Priority::Ambient };
        Some(View {
            priority,
            compact: Compact {
                icon: Icon::Music,
                text: if s.playing { s.title.clone() } else { String::new() },
                progress: None,
                pulse: false,
                toast: None,
            },
            expanded: vec![
                Row::Text { primary: s.title.clone(), secondary: Some(s.artist.clone()) },
                Row::Buttons(vec![
                    Button { control: PREV, icon: None, label: Some("⏮".into()) },
                    Button { control: PLAY, icon: None, label: Some(if s.playing { "⏸" } else { "▶" }.into()) },
                    Button { control: NEXT, icon: None, label: Some("⏭".into()) },
                ]),
            ],
        })
    }

    fn widget(&self, _now: Instant) -> Option<Widget> {
        let s = self.state.as_ref()?;
        Some(Widget::Media(MediaWidget {
            title: s.title.clone(),
            artist: s.artist.clone(),
            playing: s.playing,
            artwork: s.artwork.clone(),
            app_icon: s.app_icon.clone(),
            prev: PREV,
            play_pause: PLAY,
            next: NEXT,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(playing: bool) -> MediaState {
        MediaState {
            title: "Song".into(),
            artist: "Artist".into(),
            playing,
            source_app: "Spotify.exe".into(),
            artwork: None,
            app_icon: None,
        }
    }

    #[test]
    fn playing_is_active_paused_is_ambient() {
        let now = Instant::now();
        let mut m = MediaModule::default();
        assert!(m.on_event(&Event::Media(MediaEvent::Changed(state(true))), now));
        assert_eq!(m.view(now).unwrap().priority, Priority::Active);
        assert!(m.on_event(&Event::Media(MediaEvent::Changed(state(false))), now));
        assert_eq!(m.view(now).unwrap().priority, Priority::Ambient);
    }

    #[test]
    fn duplicate_events_are_ignored() {
        let now = Instant::now();
        let mut m = MediaModule::default();
        m.on_event(&Event::Media(MediaEvent::Changed(state(true))), now);
        assert!(!m.on_event(&Event::Media(MediaEvent::Changed(state(true))), now));
    }

    #[test]
    fn widget_exists_only_with_media_and_has_working_controls() {
        let now = Instant::now();
        let mut m = MediaModule::default();
        assert!(m.widget(now).is_none());
        m.on_event(&Event::Media(MediaEvent::Changed(state(false))), now);
        let Some(Widget::Media(w)) = m.widget(now) else { panic!("devia haver bloco de música") };
        assert_eq!((w.title.as_str(), w.playing), ("Song", false));
        let mut out = vec![];
        m.on_action(&Action { module: ID, control: w.play_pause, kind: ActionKind::Press }, now, &mut out);
        assert_eq!(out, vec![Command::MediaPlayPause]);
    }

    #[test]
    fn buttons_emit_commands() {
        let now = Instant::now();
        let mut m = MediaModule::default();
        let mut out = vec![];
        m.on_action(&Action { module: ID, control: PLAY, kind: ActionKind::Press }, now, &mut out);
        m.on_action(&Action { module: ID, control: NEXT, kind: ActionKind::Press }, now, &mut out);
        assert_eq!(out, vec![Command::MediaPlayPause, Command::MediaNext]);
    }
}
