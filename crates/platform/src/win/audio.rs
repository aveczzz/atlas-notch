//! Fonte de áudio: Core Audio (COM).
//!
//! Uma thread `audio` é dona do enumerador de dispositivos e do estado do dispositivo atual
//! (`Endpoint`). Os callbacks COM chegam em threads MTA arbitrárias e só fazem
//! `proxy.send(..)` ou mandam um sinal (`Msg`) a esta thread (regra 4). Mudanças de
//! estrutura (sessão nova ou que expira, dispositivo por omissão trocado) reconstroem o
//! `Endpoint` aqui, nunca dentro de um callback.
//!
//! - Volume geral: `IAudioEndpointVolume` + `IAudioEndpointVolumeCallback`.
//! - Dispositivo por omissão trocado (auscultadores): `IMMNotificationClient`.
//! - Por app: `IAudioSessionManager2` + `IAudioSessionNotification` (sessões novas) e, por
//!   sessão, `IAudioSessionEvents` (volume, expirou) e `ISimpleAudioVolume`.
//!
//! Os comandos usam o nosso `event_context`; os callbacks reconhecem-no e marcam o evento
//! como `quiet` (eco nosso, sem HUD). O estado inicial e a troca de dispositivo também são
//! `quiet`.

use super::process;
use crate::EventProxy;
use island_core::Event;
use island_core::event::{AudioEvent, AudioSession, SystemEvent};
use std::io;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{PROPERTYKEY, S_OK};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateExpired,
    DEVICE_STATE, EDataFlow, ERole, IAudioSessionControl, IAudioSessionControl2, IAudioSessionEvents,
    IAudioSessionEvents_Impl, IAudioSessionManager2, IAudioSessionNotification, IAudioSessionNotification_Impl,
    IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, ISimpleAudioVolume, MMDeviceEnumerator,
    eMultimedia, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, STGM_READ,
};
use windows::core::{BOOL, GUID, Interface, PCWSTR, Ref, Result, implement};

/// O nosso `event_context`: os ecos dos nossos comandos trazem-no de volta.
const OUR_CONTEXT: GUID = GUID::from_u128(0x6d1c_9a4e_7b2f_4e0c_9f61_d3a8_5e2b_7c41);
/// Sessões seguidas, no máximo (regra 6).
const MAX_SESSIONS: usize = 16;

#[derive(Debug, Clone, PartialEq)]
pub enum AudioCommand {
    Master(f32),
    SessionVolume { id: String, volume: f32 },
    SessionMute { id: String, muted: bool },
}

#[derive(Debug)]
enum Msg {
    /// Sessão nova ou expirada, ou outro dispositivo por omissão.
    Rebuild,
    /// O dispositivo de saída por omissão mudou: reconstrói e avisa com o nome.
    DeviceChanged,
    Command(AudioCommand),
}

/// Para o `exec` mandar comandos sem bloquear a thread UI.
#[derive(Debug, Clone)]
pub struct AudioControl(SyncSender<Msg>);

impl AudioControl {
    pub fn send(&self, cmd: AudioCommand) {
        let _ = self.0.try_send(Msg::Command(cmd));
    }
}

pub fn spawn(proxy: EventProxy) -> io::Result<AudioControl> {
    let (tx, rx) = sync_channel(64);
    let control = AudioControl(tx.clone());
    thread::Builder::new().name("audio".into()).spawn(move || {
        if let Err(e) = run(&rx, &tx, &proxy) {
            log!("áudio: {e}");
        }
    })?;
    Ok(control)
}

fn run(rx: &Receiver<Msg>, tx: &SyncSender<Msg>, proxy: &EventProxy) -> Result<()> {
    // SAFETY: COM (MTA) nesta thread, dona de todos os objetos de áudio abaixo.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
    // SAFETY: criação COM sem argumentos de ponteiro.
    let devices: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let watcher: IMMNotificationClient = DeviceWatcher { tx: tx.clone() }.into();
    // SAFETY: o watcher fica vivo até ao `Unregister` no fim desta função.
    unsafe { devices.RegisterEndpointNotificationCallback(&watcher)? };

    let mut endpoint = Endpoint::build(&devices, proxy, tx);
    while let Ok(msg) = rx.recv() {
        let mut device_changed = matches!(msg, Msg::DeviceChanged);
        let mut rebuild = matches!(msg, Msg::Rebuild) || device_changed;
        if let Msg::Command(cmd) = msg
            && let Some(e) = &endpoint
        {
            e.apply(&cmd);
        }
        // Rajadas (o browser cria várias sessões de seguida): uma reconstrução chega.
        while let Ok(more) = rx.try_recv() {
            match more {
                Msg::Rebuild => rebuild = true,
                Msg::DeviceChanged => {
                    rebuild = true;
                    device_changed = true;
                }
                Msg::Command(cmd) => {
                    if let Some(e) = &endpoint {
                        e.apply(&cmd);
                    }
                }
            }
        }
        if rebuild {
            // Largar primeiro: tira os registos antigos antes de criar os novos.
            drop(endpoint.take());
            endpoint = Endpoint::build(&devices, proxy, tx);
        }
        if device_changed && let Some(name) = default_device_name(&devices) {
            proxy.send(Event::System(SystemEvent::AudioDevice { name }));
        }
    }
    drop(endpoint);
    // SAFETY: o mesmo watcher registado acima.
    unsafe { devices.UnregisterEndpointNotificationCallback(&watcher)? };
    Ok(())
}

/// Nome amigável do dispositivo de saída por omissão ("Auscultadores (AirPods)").
fn default_device_name(devices: &IMMDeviceEnumerator) -> Option<String> {
    // SAFETY: COM na thread dona; a string devolvida é da API (CoTaskMem) e liberta-se aqui.
    unsafe {
        let device = devices.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let text = PropVariantToStringAlloc(&raw const value).ok()?;
        let name = text.to_string().ok();
        CoTaskMemFree(Some(text.0.cast()));
        name.filter(|n| !n.is_empty())
    }
}

/// Uma sessão de áudio seguida.
struct Tracked {
    id: String,
    control: IAudioSessionControl,
    simple: ISimpleAudioVolume,
    events: IAudioSessionEvents,
}

/// Tudo o que está registado no dispositivo atual. `Drop` tira os registos.
struct Endpoint {
    volume: IAudioEndpointVolume,
    volume_callback: IAudioEndpointVolumeCallback,
    manager: IAudioSessionManager2,
    created: IAudioSessionNotification,
    sessions: Vec<Tracked>,
}

impl Endpoint {
    /// Lê o dispositivo por omissão, regista os callbacks e manda o estado inicial (`quiet`).
    fn build(devices: &IMMDeviceEnumerator, proxy: &EventProxy, tx: &SyncSender<Msg>) -> Option<Self> {
        let built = Self::try_build(devices, proxy, tx);
        if let Err(e) = &built {
            // Sem dispositivo de saída (nenhum ligado): a island continua, sem volume.
            log!("áudio: {e}");
        }
        built.ok()
    }

    fn try_build(devices: &IMMDeviceEnumerator, proxy: &EventProxy, tx: &SyncSender<Msg>) -> Result<Self> {
        // SAFETY: chamadas COM na thread dona; os callbacks registados ficam guardados em
        // `Self` e são retirados no `Drop` antes de serem largados.
        unsafe {
            let device = devices.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;

            let volume_callback: IAudioEndpointVolumeCallback = MasterCallback { proxy: proxy.clone() }.into();
            volume.RegisterControlChangeNotify(&volume_callback)?;
            // O enumerador tem de ser pedido antes de registar a notificação de sessões novas.
            let list = manager.GetSessionEnumerator()?;
            let created: IAudioSessionNotification = SessionCreated { tx: tx.clone() }.into();
            manager.RegisterSessionNotification(&created)?;

            let mut sessions = Vec::new();
            for i in 0..list.GetCount()? {
                if sessions.len() >= MAX_SESSIONS {
                    break;
                }
                if let Ok(Some(t)) = track(list.GetSession(i)?, proxy, tx) {
                    sessions.push(t);
                }
            }
            let endpoint = Self { volume, volume_callback, manager, created, sessions };

            let snapshot: Vec<AudioSession> = endpoint.sessions.iter().filter_map(|t| t.snapshot()).collect();
            proxy.send(Event::Audio(AudioEvent::Sessions(snapshot)));
            proxy.send(Event::Audio(AudioEvent::MasterVolume {
                volume: endpoint.volume.GetMasterVolumeLevelScalar()?,
                muted: endpoint.volume.GetMute()?.as_bool(),
                quiet: true,
            }));
            Ok(endpoint)
        }
    }

    fn apply(&self, cmd: &AudioCommand) {
        let find = |id: &str| self.sessions.iter().find(|t| t.id == id);
        // SAFETY: interfaces desta thread; o GUID do contexto vive em `static`.
        let result = unsafe {
            match cmd {
                AudioCommand::Master(v) => self.volume.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), &OUR_CONTEXT),
                AudioCommand::SessionVolume { id, volume } => match find(id) {
                    Some(t) => t.simple.SetMasterVolume(volume.clamp(0.0, 1.0), &OUR_CONTEXT),
                    None => Ok(()),
                },
                AudioCommand::SessionMute { id, muted } => match find(id) {
                    Some(t) => t.simple.SetMute(*muted, &OUR_CONTEXT),
                    None => Ok(()),
                },
            }
        };
        if let Err(e) = result {
            log!("áudio: {cmd:?}: {e}");
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // SAFETY: os mesmos objetos registados em `try_build`, na thread dona.
        unsafe {
            for t in &self.sessions {
                let _ = t.control.UnregisterAudioSessionNotification(&t.events);
            }
            let _ = self.manager.UnregisterSessionNotification(&self.created);
            let _ = self.volume.UnregisterControlChangeNotify(&self.volume_callback);
        }
    }
}

/// Começa a seguir uma sessão (as expiradas ficam de fora).
fn track(control: IAudioSessionControl, proxy: &EventProxy, tx: &SyncSender<Msg>) -> Result<Option<Tracked>> {
    // SAFETY: COM na thread dona; a string do identificador é libertada com `CoTaskMemFree`.
    unsafe {
        if control.GetState()? == AudioSessionStateExpired {
            return Ok(None);
        }
        let control2: IAudioSessionControl2 = control.cast()?;
        let raw = control2.GetSessionInstanceIdentifier()?;
        let id = raw.to_string().unwrap_or_default();
        CoTaskMemFree(Some(raw.0.cast()));
        let simple: ISimpleAudioVolume = control.cast()?;
        let events: IAudioSessionEvents = SessionEvents { id: id.clone(), proxy: proxy.clone(), tx: tx.clone() }.into();
        control.RegisterAudioSessionNotification(&events)?;
        Ok(Some(Tracked { id, control, simple, events }))
    }
}

impl Tracked {
    fn snapshot(&self) -> Option<AudioSession> {
        // SAFETY: interfaces desta thread.
        unsafe {
            let control2: IAudioSessionControl2 = self.control.cast().ok()?;
            let name = if control2.IsSystemSoundsSession() == S_OK {
                "Sons do sistema".to_owned()
            } else {
                let pid = control2.GetProcessId().ok()?;
                process::image_path(pid).map(|p| process::display_name(&p)).unwrap_or_else(|| format!("PID {pid}"))
            };
            Some(AudioSession {
                id: self.id.clone(),
                name,
                volume: self.simple.GetMasterVolume().ok()?,
                muted: self.simple.GetMute().ok()?.as_bool(),
            })
        }
    }
}

/// Volume geral mudou (teclas, outra app, ou eco nosso).
#[implement(IAudioEndpointVolumeCallback)]
struct MasterCallback {
    proxy: EventProxy,
}

impl IAudioEndpointVolumeCallback_Impl for MasterCallback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        // SAFETY: o Core Audio garante `data` válido durante a chamada (pode vir nulo).
        if let Some(d) = unsafe { data.as_ref() } {
            self.proxy.send(Event::Audio(AudioEvent::MasterVolume {
                volume: d.fMasterVolume,
                muted: d.bMuted.as_bool(),
                quiet: d.guidEventContext == OUR_CONTEXT,
            }));
        }
        Ok(())
    }
}

/// Dispositivo de saída por omissão trocado: reconstruir.
#[implement(IMMNotificationClient)]
struct DeviceWatcher {
    tx: SyncSender<Msg>,
}

impl IMMNotificationClient_Impl for DeviceWatcher_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> Result<()> {
        if flow == eRender && role == eMultimedia {
            let _ = self.tx.try_send(Msg::DeviceChanged);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

/// Sessão nova (uma app começou a tocar).
#[implement(IAudioSessionNotification)]
struct SessionCreated {
    tx: SyncSender<Msg>,
}

impl IAudioSessionNotification_Impl for SessionCreated_Impl {
    fn OnSessionCreated(&self, _: Ref<IAudioSessionControl>) -> Result<()> {
        let _ = self.tx.try_send(Msg::Rebuild);
        Ok(())
    }
}

/// Mudanças numa sessão.
#[implement(IAudioSessionEvents)]
struct SessionEvents {
    id: String,
    proxy: EventProxy,
    tx: SyncSender<Msg>,
}

impl IAudioSessionEvents_Impl for SessionEvents_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnSimpleVolumeChanged(&self, volume: f32, muted: BOOL, _: *const GUID) -> Result<()> {
        self.proxy.send(Event::Audio(AudioEvent::SessionVolume { id: self.id.clone(), volume, muted: muted.as_bool() }));
        Ok(())
    }
    fn OnChannelVolumeChanged(&self, _: u32, _: *const f32, _: u32, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn OnStateChanged(&self, state: AudioSessionState) -> Result<()> {
        if state == AudioSessionStateExpired {
            let _ = self.tx.try_send(Msg::Rebuild);
        }
        Ok(())
    }
    fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> Result<()> {
        let _ = self.tx.try_send(Msg::Rebuild);
        Ok(())
    }
}
