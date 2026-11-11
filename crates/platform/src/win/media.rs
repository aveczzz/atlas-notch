//! Fonte de media: `GlobalSystemMediaTransportControlsSessionManager` (GSMTC).
//!
//! Uma thread "media" é dona do manager e da sessão atual. Os handlers do WinRT correm em
//! threads do pool e **só** mandam um sinal (`Msg`) para esta thread (regra 4); é ela que faz
//! as leituras bloqueantes (`join`) e os comandos. Em repouso dorme no `recv` do canal.
//!
//! - `CurrentSessionChanged` no manager; na sessão atual, `MediaPropertiesChanged` e
//!   `PlaybackInfoChanged`. Ressubscrever quando a sessão muda.
//! - A capa (`Thumbnail`) só se pede quando título/artista mudam, e tem tamanho máximo.
//! - Comandos (`TryTogglePlayPauseAsync`, ...) chegam do `exec` pelo mesmo canal e são
//!   fire-and-forget: a confirmação chega pelos eventos.

use super::appicon;
use crate::EventProxy;
use island_core::event::{ImageData, MediaEvent, MediaState};
use island_core::Event;
use std::io;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;
use windows::Foundation::TypedEventHandler;
use windows_future::IAsyncOperation;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionMediaProperties as Properties,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::{Error, Interface, Result};

/// Capas maiores do que isto ficam de fora (regra 6). As normais têm 10-300 KB.
const MAX_ARTWORK: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

#[derive(Debug)]
enum Msg {
    SessionChanged,
    Refresh,
    Command(MediaCommand),
}

/// Para o `exec` mandar comandos à sessão atual sem bloquear a thread UI.
#[derive(Debug, Clone)]
pub struct MediaControl(SyncSender<Msg>);

impl MediaControl {
    pub fn send(&self, cmd: MediaCommand) {
        let _ = self.0.try_send(Msg::Command(cmd));
    }
}

pub fn spawn(proxy: EventProxy) -> io::Result<MediaControl> {
    // Sinais e comandos: poucos e pequenos. Cheio = já há um refresh pendente, perder não faz mal.
    let (tx, rx) = sync_channel(32);
    let control = MediaControl(tx.clone());
    thread::Builder::new().name("media".into()).spawn(move || {
        if let Err(e) = run(&rx, &tx, &proxy) {
            log!("media: {e}");
        }
    })?;
    Ok(control)
}

/// Sinal a partir de um handler WinRT: nunca bloqueia.
fn signal(tx: &SyncSender<Msg>, msg: Msg) {
    if let Err(TrySendError::Disconnected(_)) = tx.try_send(msg) {
        // A thread media terminou; nada a fazer.
    }
}

struct Current {
    session: Session,
    tokens: (i64, i64),
}

impl Current {
    fn unsubscribe(&self) {
        let _ = self.session.RemoveMediaPropertiesChanged(self.tokens.0);
        let _ = self.session.RemovePlaybackInfoChanged(self.tokens.1);
    }
}

fn run(rx: &Receiver<Msg>, tx: &SyncSender<Msg>, proxy: &EventProxy) -> Result<()> {
    // SAFETY: inicializa COM (MTA) nesta thread, que é dona de todos os objetos WinRT abaixo.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
    let manager = Manager::RequestAsync()?.join()?;
    let t = tx.clone();
    let _changed = manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        signal(&t, Msg::SessionChanged);
        Ok(())
    }))?;

    let mut current: Option<Current> = None;
    let mut images = Images::default();
    signal(tx, Msg::SessionChanged);

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::SessionChanged => {
                if let Some(c) = current.take() {
                    c.unsubscribe();
                }
                current = manager.GetCurrentSession().ok().and_then(|s| subscribe(s, tx).ok());
            }
            Msg::Refresh => {}
            Msg::Command(cmd) => {
                if let Some(c) = &current {
                    // Fire-and-forget: o resultado chega por `PlaybackInfoChanged`.
                    let _ = match cmd {
                        MediaCommand::PlayPause => c.session.TryTogglePlayPauseAsync(),
                        MediaCommand::Next => c.session.TrySkipNextAsync(),
                        MediaCommand::Previous => c.session.TrySkipPreviousAsync(),
                    };
                }
                continue;
            }
        }
        // Vários sinais seguidos (o GSMTC repete muito): um refresh chega para todos.
        let mut session_changed = false;
        while let Ok(extra) = rx.try_recv() {
            match extra {
                Msg::SessionChanged => session_changed = true,
                Msg::Refresh => {}
                Msg::Command(cmd) => {
                    let _ = tx.try_send(Msg::Command(cmd));
                    break;
                }
            }
        }
        if session_changed {
            if let Some(c) = current.take() {
                c.unsubscribe();
            }
            current = manager.GetCurrentSession().ok().and_then(|s| subscribe(s, tx).ok());
        }

        let event = match &current {
            Some(c) => match read_state(&c.session, &mut images) {
                Ok(Some(state)) => MediaEvent::Changed(state),
                Ok(None) => MediaEvent::Cleared,
                Err(e) => {
                    log!("media: {e}");
                    continue;
                }
            },
            None => MediaEvent::Cleared,
        };
        if !proxy.send(Event::Media(event)) {
            break;
        }
    }
    if let Some(c) = current {
        c.unsubscribe();
    }
    Ok(())
}

fn subscribe(session: Session, tx: &SyncSender<Msg>) -> Result<Current> {
    let (a, b) = (tx.clone(), tx.clone());
    let props = session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
        signal(&a, Msg::Refresh);
        Ok(())
    }))?;
    let playback = session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
        signal(&b, Msg::Refresh);
        Ok(())
    }))?;
    signal(tx, Msg::Refresh);
    Ok(Current { session, tokens: (props, playback) })
}

/// Imagens já obtidas: só se pedem outra vez quando a música ou a app mudam.
#[derive(Default)]
struct Images {
    /// Título/artista da capa atual.
    art_key: Option<(String, String)>,
    artwork: Option<ImageData>,
    /// AUMID do ícone atual.
    icon_key: Option<String>,
    icon: Option<ImageData>,
}

/// Estado atual da sessão, ou `None` se não tem nada a mostrar (sem título).
fn read_state(session: &Session, images: &mut Images) -> Result<Option<MediaState>> {
    let props = session.TryGetMediaPropertiesAsync().and_then(|op| op.join()).map_err(stage("pedir as propriedades da música"))?;
    let title = props.Title()?.to_string_lossy();
    if title.trim().is_empty() {
        return Ok(None);
    }
    let artist = props.Artist()?.to_string_lossy();
    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == Status::Playing;
    let source_app = session.SourceAppUserModelId()?.to_string_lossy();
    let key = (title.clone(), artist.clone());
    if images.art_key.as_ref() != Some(&key) {
        images.artwork = read_artwork(&props).unwrap_or_else(|e| {
            log!("media: capa de \"{title}\": {e}");
            None
        });
        images.art_key = Some(key);
    }
    if images.icon_key.as_ref() != Some(&source_app) {
        images.icon = appicon::for_app(&source_app);
        images.icon_key = Some(source_app.clone());
    }
    Ok(Some(MediaState {
        title,
        artist,
        playing,
        source_app,
        artwork: images.artwork.clone(),
        app_icon: images.icon.clone(),
    }))
}

/// Acrescenta ao erro o passo em que falhou, para o `island.log` dizer onde (o código do
/// Windows sozinho engana: um `0x80030070` diz "disco cheio" sem o disco ter nada a ver).
fn stage(what: &'static str) -> impl Fn(Error) -> Error {
    move |e| Error::new(e.code(), format!("{what}: {}", e.message()))
}

/// Bytes da capa (PNG/JPEG). `Ok(None)` se a música não tem capa ou ela é absurda; `Err` com o
/// passo que falhou se o Windows não a deu.
fn read_artwork(props: &Properties) -> Result<Option<ImageData>> {
    // Sem miniatura é o normal (rádio, alguns sites): não é erro.
    let Ok(reference) = props.Thumbnail() else { return Ok(None) };
    let stream = reference.OpenReadAsync().and_then(|op| op.join()).map_err(stage("abrir a miniatura"))?;
    let size = stream.Size().map_err(stage("tamanho da miniatura"))?;
    if size == 0 || size > MAX_ARTWORK {
        return Ok(None);
    }
    let reader = DataReader::CreateDataReader(&stream).map_err(stage("ler a miniatura"))?;
    // O `LoadAsync` pode trazer MENOS bytes do que os pedidos (streams lentos, como as
    // miniaturas dos browsers): lê-se até ter tudo ou o stream acabar.
    let mut bytes = Vec::with_capacity(size as usize);
    while (bytes.len() as u64) < size {
        let falta = (size - bytes.len() as u64) as u32;
        let load: IAsyncOperation<u32> = reader.LoadAsync(falta).map_err(stage("ler a miniatura"))?.cast()?;
        let loaded = load.join().map_err(stage("ler a miniatura"))?;
        if loaded == 0 {
            break;
        }
        let mut chunk = vec![0u8; loaded as usize];
        reader.ReadBytes(&mut chunk).map_err(stage("ler a miniatura"))?;
        bytes.extend_from_slice(&chunk);
    }
    if (bytes.len() as u64) < size {
        log!("media: capa incompleta ({} de {size} bytes): fica sem capa", bytes.len());
        return Ok(None);
    }
    Ok(Some(ImageData(bytes.into())))
}
