//! Backend Windows. Fase 1 feita (janela + loop); o resto é **esqueleto**: as assinaturas
//! e a ordem de implementação estão fixas, os corpos são `todo!()` com as APIs exatas a
//! usar. Ver o roadmap no CLAUDE.md.
//!
//! Event loop (thread UI):
//!
//! ```text
//! loop {
//!     timeout = se a animar: até ao próximo frame do compositor
//!               senão se engine.next_deadline(): até lá
//!               senão: INFINITE
//!     MsgWaitForMultipleObjectsEx(0, [], timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
//!     drenar PeekMessageW (WM_APP_WAKE, rato, DPI, ...)
//!     drenar rx: engine.dispatch(ev, now)
//!     se deadline passou: engine.on_deadline(now)
//!     exec::run(engine.drain_commands())
//!     se redraw ou a animar: renderer.frame(dt, engine.current(), expanded)
//! }
//! ```
//!
//! Em repouso isto fica bloqueado no kernel: 0% CPU, zero wakeups.

mod appicon;
mod audio;
mod backdrop;
mod dragdrop;
mod exec;
mod fullscreen;
mod keys;
mod notifyicon;
mod geometry;
mod ipc;
mod media;
mod notifications;
mod process;
mod panel;
mod render;
mod settings;
mod sysevents;
mod window;

use crate::config::{self, Config};
use crate::{EventProxy, headless};
use island_core::event::TrayEvent;
use island_core::view::Widget;
use island_core::{Action, ActionKind, ControlId, Engine, Event, ModuleId};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use std::thread;
use std::time::{Duration, Instant};
use panel::{Hit, Tab};
use window::Input;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WAIT_FAILED, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Ole::{DROPEFFECT_MOVE, OleInitialize, OleUninitialize};
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, PostMessageW,
    QS_ALLINPUT, TranslateMessage, WM_APP, WM_QUIT,
};

/// Mensagem custom que acorda a thread UI quando há eventos no canal.
pub const WM_APP_WAKE: u32 = WM_APP + 1;

/// `dev_stdin`: lê também mensagens do protocolo pelo stdin (desenvolvimento, ver `headless`).
pub fn run(mut engine: Engine, dev_stdin: bool) -> io::Result<()> {
    // SAFETY: chamado antes de criar qualquer janela. Falha se o DPI já foi definido
    // (manifest, compatibilidade): fica o que estiver, não é fatal.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // SAFETY: OLE (inclui COM STA) na thread UI, que bombeia mensagens como um STA exige.
    // OLE e não só COM: o arrastar e largar (`dragdrop`) precisa dele.
    unsafe { OleInitialize(None)? };
    // Com `panic = "abort"` a app fecha sem dizer nada: o gancho deixa o motivo no log.
    std::panic::set_hook(Box::new(|info| log!("PÂNICO: {info}")));
    let attempt = relaunch_attempt();
    log!("a arrancar (v{}, pid {}, tentativa {})", env!("CARGO_PKG_VERSION"), std::process::id(), attempt + 1);
    let started = Instant::now();
    let result = run_loop(&mut engine, dev_stdin);
    // SAFETY: emparelha com o `OleInitialize` acima, na mesma thread.
    unsafe { OleUninitialize() };
    match &result {
        Ok(()) => log!("a sair normalmente (esteve {} s a correr)", started.elapsed().as_secs()),
        Err(e) => {
            log!("ERRO FATAL ao fim de {} s: {e}", started.elapsed().as_secs());
            // Falhas no arranque do Windows (ecrã ou GPU ainda não prontos) costumam passar em
            // segundos: tenta outra vez, noutro processo, em vez de ficar sem island.
            // "Já há outra island" não é uma falha a repetir.
            if e.kind() != io::ErrorKind::AlreadyExists && attempt < MAX_RELAUNCH && relaunch(attempt + 1) {
                log!("relançada daqui a {} s", RELAUNCH_DELAY.as_secs());
            }
        }
    }
    result
}

/// Quantas vezes a island se relança sozinha depois de uma falha.
const MAX_RELAUNCH: u32 = 5;
const RELAUNCH_DELAY: Duration = Duration::from_secs(4);
const RELAUNCH_ENV: &str = "ISLAND_RELAUNCH";

fn relaunch_attempt() -> u32 {
    std::env::var(RELAUNCH_ENV).ok().and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// Arranca outra cópia desta app com a contagem de tentativas. A pausa é feita no processo
/// novo (nunca aqui, a segurar o pipe).
fn relaunch(next_attempt: u32) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    std::process::Command::new(exe)
        .env(RELAUNCH_ENV, next_attempt.to_string())
        .args(std::env::args().skip(1))
        .spawn()
        .is_ok()
}

fn run_loop(engine: &mut Engine, dev_stdin: bool) -> io::Result<()> {
    // Um relançamento começa com uma pausa: dá tempo ao ecrã e à GPU para ficarem prontos.
    if relaunch_attempt() > 0 {
        thread::sleep(RELAUNCH_DELAY);
    }
    // Antes de tudo: se outra island já tem o pipe, esta sai sem abrir janela.
    let server = ipc::bind()?;
    let placement = window::placement()?;
    let hwnd = window::create(placement)?;
    let mut renderer = render::Renderer::new(hwnd, placement)?;
    // Primeiro frame e região antes de mostrar: nunca aparece um retângulo vazio.
    let first = renderer.frame(0.0, engine.current(), engine.panel(), false, Tab::Nook, false);
    if let Some(points) = &first.region {
        window::set_region(hwnd, points)?;
    }
    window::show(hwnd);
    window::register_hotkey(hwnd);
    window::register_system(hwnd);
    notifyicon::register(hwnd);
    fullscreen::register(hwnd);
    if let Err(e) = dragdrop::register(hwnd) {
        log!("tray: sem arrastar e largar: {e}");
    }
    let hwnd_raw = hwnd.0 as isize;

    let (tx, rx) = crate::proxy::channel();
    // Um wakeup chega para drenar tudo o que estiver no canal. Sem isto, cada evento era
    // um `PostMessageW` e uma rajada podia encher a fila de mensagens (limite de 10 000).
    let wake_pending = Arc::new(AtomicBool::new(false));
    let proxy = EventProxy::new(tx, {
        let pending = Arc::clone(&wake_pending);
        Arc::new(move || {
            if !pending.swap(true, Ordering::AcqRel) {
                // SAFETY: `PostMessageW` pode ser chamado de qualquer thread. Se a janela já
                // não existir, falha e ignora-se (a thread UI está a sair).
                let _ = unsafe { PostMessageW(Some(HWND(hwnd_raw as _)), WM_APP_WAKE, WPARAM(0), LPARAM(0)) };
            }
        })
    });

    server.spawn(proxy.clone())?;
    // Teclas de bloqueio: uma thread própria (raw input). Falhar só desliga estes avisos.
    if let Err(e) = keys::spawn(proxy.clone()) {
        log!("teclas de bloqueio: {e}");
    }
    // Uma fonte que falhe (ex.: GSMTC indisponível) não impede a island de correr.
    let sinks = exec::Sinks {
        media: media::spawn(proxy.clone()).map_err(|e| log!("media: {e}")).ok(),
        audio: audio::spawn(proxy.clone()).map_err(|e| log!("áudio: {e}")).ok(),
        notifications: notifications::spawn(proxy.clone()).map_err(|e| log!("notificações: {e}")).ok(),
    };
    if dev_stdin {
        let proxy = proxy.clone();
        thread::Builder::new()
            .name("stdin-ipc".into())
            .spawn(move || headless::read_ipc(io::stdin().lock(), &proxy))?;
    }

    // Sem guardar o lock do stderr: outras threads (pipe, fontes COM) também escrevem lá,
    // e um `stderr().lock()` vivo no loop bloqueava-as para sempre.
    let _ = headless::print_view(&mut io::stderr(), engine.current());

    // Configuração: lida agora e de novo sempre que a pasta muda (o handle entra na espera).
    let cfg_dir = config::dir().map_err(|e| log!("config: {e}")).ok();
    let mut cfg = cfg_dir.as_deref().map(Config::load).unwrap_or_default();
    settings::apply_autostart(cfg.autostart);
    renderer.set_glass(cfg.glass);
    let watcher = cfg_dir.as_deref().and_then(settings::Watcher::new);
    // Limites da conta: o último valor que o `island-hook statusline` guardou (para mostrar ao
    // arrancar, mesmo que a island estivesse fechada). Com ela aberta chegam ao vivo pelo pipe.
    if let Some(u) = crate::usage::load_saved() {
        engine.dispatch(Event::External(island_proto::Message::ClaudeUsage(u)), Instant::now());
    }
    // A Tray da última sessão. Os ícones vêm do Shell, fora da thread UI, como ao largar.
    let mut saved_tray = Vec::new();
    if let Some(dir) = cfg_dir.as_deref().filter(|_| cfg.keep_tray) {
        saved_tray = config::load_tray(dir);
        if !saved_tray.is_empty() {
            add_tray_files(&proxy, saved_tray.clone());
        }
    }
    // Até a lista reposta chegar, a Tray do painel está vazia e não pode ser gravada por cima.
    let mut tray_restoring = !saved_tray.is_empty();

    let mut msg = MSG::default();
    let mut ui = UiState::default();
    loop {
        let now = Instant::now();
        let timeout = if ui.animating {
            0 // O ritmo vem do `DwmFlush` no fim da iteração.
        } else {
            [engine.next_deadline(), ui.collapse_at, ui.retry_at, ui.tab_at.map(|(_, at)| at)]
                .into_iter()
                .flatten()
                .min()
                .map_or(INFINITE, |d| timeout_ms(d, now))
        };
        // Handles na espera: o evento de device lost do renderer e a vigia da configuração.
        let lost = renderer.lost_event();
        let mut handles = [HANDLE::default(); 2];
        let mut n = 0;
        let lost_at = lost.map(|h| {
            handles[n] = h;
            n += 1;
            n - 1
        });
        let watch_at = watcher.as_ref().map(|w| {
            handles[n] = w.handle();
            n += 1;
            n - 1
        });
        let handles = (n > 0).then(|| &handles[..n]);
        // SAFETY: os handles são do renderer e da vigia, vivos durante a espera.
        let woke = unsafe { MsgWaitForMultipleObjectsEx(handles, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        if woke == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        let signaled = woke.0.checked_sub(WAIT_OBJECT_0.0).map(|i| i as usize);
        let mut redraw = false;
        if lost_at.is_some() && signaled == lost_at {
            // O driver gráfico reiniciou (ou a GPU mudou): recriar tudo e redesenhar.
            renderer.device_lost();
            redraw = true;
        }
        if let (Some(w), Some(dir)) = (&watcher, cfg_dir.as_deref())
            && watch_at.is_some()
            && signaled == watch_at
        {
            w.rearm();
            let new = Config::load(dir);
            if new != cfg {
                if new.autostart != cfg.autostart {
                    settings::apply_autostart(new.autostart);
                }
                if !new.hide_in_fullscreen && ui.fullscreen {
                    window::push(Input::Fullscreen(false));
                }
                if new.glass != cfg.glass {
                    renderer.set_glass(new.glass);
                    redraw = true;
                }
                cfg = new;
            }
        }

        // SAFETY: `msg` é local e só é lido depois de o `PeekMessageW` o preencher.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                return Ok(());
            }
            // SAFETY: `msg` acabou de vir da fila desta thread.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // Tem de ser um `swap` (leitura-escrita), não um `store`: sincroniza com o `swap`
        // de quem enviou e garante que o `try_recv` a seguir vê o evento. Um evento que
        // chegue depois volta a acordar o loop.
        wake_pending.swap(false, Ordering::AcqRel);
        let now = Instant::now();
        for input in window::take_input() {
            match input {
                // Só a forma visível conta: a encolher, a região da janela ainda tem o tamanho
                // do painel, e passar onde ele *estava* não pode voltar a abri-lo.
                Input::Hover { x, y } => {
                    let inside = renderer.shape_contains(x, y);
                    if inside && !ui.hovered {
                        ui.hovered = true;
                        ui.collapse_at = None;
                    } else if !inside && ui.hovered && ui.dragging.is_none() {
                        ui.hovered = false;
                        ui.collapse_at = Some(now + COLLAPSE_DELAY);
                    }
                    // Passar por cima de um separador abre-o, depois de uma pequena espera (para não
                    // trocar ao atravessá-los a caminho de outra coisa). O prazo entra no timeout do loop.
                    match renderer.hit_test(x, y) {
                        Some(Hit::Tab(tab)) if tab != ui.tab => {
                            if ui.tab_at.is_none_or(|(t, _)| t != tab) {
                                ui.tab_at = Some((tab, now + TAB_DWELL));
                            }
                        }
                        _ => ui.tab_at = None,
                    }
                }
                Input::Leave => {
                    ui.tab_at = None;
                    // Só se estava dentro da forma: sair da região sem lá ter entrado não
                    // pode marcar um "encolher", que com `expanded = hovered || collapse_at`
                    // abria a ilha por um instante. A arrastar um slider, só encolhe ao largar.
                    if ui.hovered && ui.dragging.is_none() {
                        ui.collapse_at = Some(now + COLLAPSE_DELAY);
                    }
                    ui.hovered = false;
                }
                Input::Moved(p) => {
                    renderer.moved(p);
                    redraw = true;
                }
                Input::Press { x, y } => match renderer.hit_test(x, y) {
                    Some(Hit::Slider(module, control, track)) => {
                        window::capture(hwnd, true);
                        ui.dragging = Some((module, control, track));
                        redraw |= slide(engine, &renderer, ui.dragging, x, y, now);
                    }
                    // Um ficheiro da Tray: ainda não se sabe se é clique (abrir) ou arrastar.
                    Some(Hit::File { module, open, remove, path }) => {
                        window::capture(hwnd, true);
                        ui.file_press = Some(FilePress { module, open, remove, path, x, y });
                    }
                    _ => {}
                },
                Input::Drag { x, y } => {
                    redraw |= slide(engine, &renderer, ui.dragging, x, y, now);
                    if ui.file_press.as_ref().is_some_and(|p| (p.x - x).abs() + (p.y - y).abs() > DRAG_START_PX)
                        && let Some(p) = ui.file_press.take()
                    {
                        // Para fora: modal até largar. Se o ficheiro foi movido, sai da lista.
                        window::capture(hwnd, false);
                        if dragdrop::drag_out(&p.path) == DROPEFFECT_MOVE {
                            let action = Action { module: p.module, control: p.remove, kind: ActionKind::Press };
                            redraw |= engine.dispatch(Event::Action(action), now);
                        }
                    }
                }
                Input::Click { .. } if ui.dragging.is_some() => {
                    ui.dragging = None;
                    window::capture(hwnd, false);
                    if !ui.hovered {
                        ui.collapse_at = Some(now + COLLAPSE_DELAY);
                    }
                }
                // Carregou num ficheiro e largou sem arrastar: abrir.
                Input::Click { .. } if ui.file_press.is_some() => {
                    window::capture(hwnd, false);
                    if let Some(p) = ui.file_press.take() {
                        let action = Action { module: p.module, control: p.open, kind: ActionKind::Press };
                        redraw |= engine.dispatch(Event::Action(action), now);
                    }
                }
                // Ficheiros arrastados por cima: abrir já no separador Tray.
                // Sem mudança nenhuma (o caso normal a cada clique noutra janela): não mexer em nada.
                Input::Fullscreen(on) if ui.fullscreen == (on && cfg.hide_in_fullscreen) => {}
                Input::ToggleHidden | Input::Fullscreen(_) => {
                    match input {
                        Input::Fullscreen(on) => ui.fullscreen = on && cfg.hide_in_fullscreen,
                        _ => ui.hidden = !ui.hidden,
                    }
                    window::set_visible(hwnd, ui.visible());
                    // Ao voltar, aparece fechada e redesenha o estado atual.
                    ui.hovered = false;
                    ui.collapse_at = None;
                    ui.animating = false;
                    redraw = true;
                }
                Input::DragEnter => {
                    ui.drop_hover = true;
                    ui.collapse_at = None;
                    ui.tab = Tab::Tray;
                    redraw = true;
                }
                Input::DragLeave => {
                    ui.drop_hover = false;
                    if !ui.hovered {
                        ui.collapse_at = Some(now + COLLAPSE_DELAY);
                    }
                    redraw = true;
                }
                Input::Dropped(paths) => {
                    ui.drop_hover = false;
                    // Fica aberta um pouco para se ver o ficheiro a chegar.
                    ui.collapse_at = Some(now + SHOW_AFTER_DROP);
                    redraw = true;
                    if !paths.is_empty() {
                        add_tray_files(&proxy, paths);
                    }
                }
                Input::OpenConfig => match cfg_dir.as_deref().map(config::ensure_file) {
                    Some(Ok(path)) => settings::open_config(&path),
                    Some(Err(e)) => log!("config: {e}"),
                    None => {}
                },
                Input::System(event) => {
                    if cfg.system_alerts {
                        redraw |= engine.dispatch(Event::System(event), now);
                    }
                }
                Input::Click { x, y } => match renderer.hit_test(x, y) {
                    Some(Hit::Control(module, control)) => {
                        let action = Action { module, control, kind: ActionKind::Press };
                        redraw |= engine.dispatch(Event::Action(action), now);
                    }
                    Some(Hit::Tab(tab)) if tab != ui.tab => {
                        ui.tab = tab;
                        redraw = true;
                    }
                    Some(Hit::Gear) => match cfg_dir.as_deref().map(config::ensure_file) {
                        Some(Ok(path)) => settings::open_config(&path),
                        Some(Err(e)) => log!("config: {e}"),
                        None => {}
                    },
                    Some(Hit::Tab(_) | Hit::Slider(..) | Hit::File { .. }) | None => {}
                },
            }
        }
        if let Some((tab, at)) = ui.tab_at
            && at <= now
        {
            ui.tab_at = None;
            if ui.tab != tab {
                ui.tab = tab;
                redraw = true;
            }
        }
        if ui.collapse_at.is_some_and(|t| t <= now) {
            ui.collapse_at = None;
        }
        if ui.retry_at.is_some_and(|t| t <= now) {
            ui.retry_at = None;
            redraw = true;
        }
        // Com um toast à mostra o rato não abre o painel (o clique abre a app); largar ficheiros sim.
        let expanded = ((ui.hovered || ui.collapse_at.is_some()) && !renderer.toast_showing()) || ui.drop_hover;
        if expanded != ui.expanded {
            ui.expanded = expanded;
            redraw = true;
            // Os blocos dependem da hora (durações, quando um limite reinicia): ao abrir, refresca.
            if expanded {
                redraw |= engine.refresh(Instant::now());
            }
        }
        while let Ok(event) = rx.try_recv() {
            if matches!(event, Event::System(_)) && !cfg.system_alerts {
                continue;
            }
            redraw |= engine.dispatch(event, now);
        }
        if engine.next_deadline().is_some_and(|d| d <= now) {
            redraw |= engine.on_deadline(now);
        }
        exec::run(engine.drain_commands(), &sinks);
        // A lista da Tray mudou? Guardar (só quando o painel muda, e só se mudou mesmo).
        if redraw && cfg.keep_tray && let Some(dir) = cfg_dir.as_deref() {
            let tray = tray_paths(engine.panel());
            tray_restoring &= tray.is_empty();
            if !tray_restoring && tray != saved_tray {
                if let Err(e) = config::save_tray(dir, &tray) {
                    log!("tray: não consegui guardar a lista: {e}");
                }
                saved_tray = tray;
            }
        }
        // Escondida: o estado continua a andar (eventos, deadlines), só não se desenha.
        if (redraw || ui.animating) && ui.visible() {
            let now = Instant::now();
            // Ao começar uma animação, um frame; depois, o tempo real entre frames.
            let dt = if ui.animating { (now - ui.last_frame).as_secs_f32() } else { FRAME };
            ui.last_frame = now;
            let out = renderer.frame(dt, engine.current(), engine.panel(), ui.expanded, ui.tab, ui.drop_hover);
            if let Some(points) = &out.region {
                let _ = window::set_region(hwnd, points);
            }
            ui.animating = out.animating;
            if out.retry {
                ui.retry_at = Some(now + RETRY_DELAY);
            }
            if ui.animating {
                wait_next_frame(now);
            }
        }
    }
}

/// Junta ficheiros à Tray. Os ícones vêm do Shell (dezenas de ms por ficheiro): fora da thread UI.
fn add_tray_files(proxy: &EventProxy, paths: Vec<String>) {
    let proxy = proxy.clone();
    let _ = thread::Builder::new().name("tray-icons".into()).spawn(move || {
        // SAFETY: COM nesta thread curta, para os pedidos ao Shell.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        proxy.send(Event::Tray(TrayEvent::Added(dragdrop::describe(&paths))));
    });
}

/// Os caminhos na Tray, pela ordem em que aparecem (mais recente primeiro).
fn tray_paths(panel: &[(ModuleId, Widget)]) -> Vec<String> {
    panel
        .iter()
        .find_map(|(_, w)| match w {
            Widget::Tray(t) => Some(t.items.iter().map(|i| i.path.clone()).collect()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Valor do slider a ser arrastado na posição do rato -> `ActionKind::SetValue`.
fn slide(
    engine: &mut Engine,
    renderer: &render::Renderer,
    dragging: Option<(ModuleId, ControlId, geometry::Rect)>,
    x: i32,
    y: i32,
    now: Instant,
) -> bool {
    let Some((module, control, track)) = dragging else { return false };
    let (px, _) = renderer.panel_point(x, y);
    let value = geometry::slider_value(&track, px);
    engine.dispatch(Event::Action(Action { module, control, kind: ActionKind::SetValue(value) }), now)
}

/// Bloqueia até à composição seguinte do DWM, para os frames ficarem alinhados com o ecrã.
/// Em ecrãs rápidos (144 Hz) salta composições até passar `MIN_FRAME`: a 144 frames/s o
/// WARP gastava o dobro do CPU sem diferença visível numa animação de meio segundo.
/// No máximo 3 esperas, caso o DWM devolva logo (sem composição pendente).
fn wait_next_frame(frame_start: Instant) {
    for _ in 0..3 {
        // SAFETY: sem argumentos.
        if unsafe { DwmFlush() }.is_err() || frame_start.elapsed() >= MIN_FRAME {
            break;
        }
    }
}

/// ~80 frames/s no máximo durante animações.
const MIN_FRAME: Duration = Duration::from_millis(12);

/// Ficheiro da Tray premido, à espera de saber se é clique ou arrastar.
#[derive(Debug)]
struct FilePress {
    module: ModuleId,
    open: ControlId,
    remove: ControlId,
    path: String,
    x: i32,
    y: i32,
}

/// Pixeis de movimento com o botão premido a partir dos quais é arrastar, não clique.
const DRAG_START_PX: i32 = 6;
/// Depois de largar ficheiros, a island fica aberta este tempo para se ver o que chegou.
const SHOW_AFTER_DROP: Duration = Duration::from_millis(1500);

/// Encolher só um pouco depois de o rato sair, para não piscar ao passar na borda.
const COLLAPSE_DELAY: Duration = Duration::from_millis(250);
/// O rato tem de ficar este tempo em cima de um separador para ele abrir.
const TAB_DWELL: Duration = Duration::from_millis(120);
/// Voltar a tentar criar a cadeia gráfica (driver a reiniciar).
const RETRY_DELAY: Duration = Duration::from_millis(500);
const FRAME: f32 = 1.0 / 60.0;

/// Estado de apresentação que pertence à plataforma, não aos módulos.
#[derive(Debug)]
struct UiState {
    hovered: bool,
    /// O rato saiu: encolher a esta hora (entra no timeout do loop, sem polling).
    collapse_at: Option<Instant>,
    /// Separador sob o rato e a que horas abre (hover).
    tab_at: Option<(Tab, Instant)>,
    expanded: bool,
    animating: bool,
    last_frame: Instant,
    retry_at: Option<Instant>,
    /// Ficheiro da Tray premido (abre ao largar, ou arrasta-se para fora).
    file_press: Option<FilePress>,
    /// Escondida pelo utilizador (Ctrl+Alt+I ou menu): não desenha nem reage ao rato.
    hidden: bool,
    /// Uma app em ecrã inteiro no monitor da island: esconde sem mexer em `hidden`.
    fullscreen: bool,
    /// Ficheiros a ser arrastados por cima da island.
    drop_hover: bool,
    /// Slider a ser arrastado: módulo, controlo e a barra (DIPs).
    dragging: Option<(ModuleId, ControlId, geometry::Rect)>,
    /// Separador aberto no painel.
    tab: Tab,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            hovered: false,
            collapse_at: None,
            tab_at: None,
            expanded: false,
            animating: false,
            last_frame: Instant::now(),
            retry_at: None,
            tab: Tab::Nook,
            dragging: None,
            file_press: None,
            drop_hover: false,
            hidden: false,
            fullscreen: false,
        }
    }
}

impl UiState {
    fn visible(&self) -> bool {
        !self.hidden && !self.fullscreen
    }
}

/// Milissegundos até `deadline`, para o `MsgWaitForMultipleObjectsEx`.
///
/// Arredonda para cima: para baixo, acordava antes do deadline e o loop rodava em vazio
/// até lá. Satura abaixo de `INFINITE`, que quer dizer "para sempre".
fn timeout_ms(deadline: Instant, now: Instant) -> u32 {
    let ms = deadline.saturating_duration_since(now).as_nanos().div_ceil(1_000_000);
    ms.min(u128::from(INFINITE - 1)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timeout_rounds_up_and_saturates() {
        let now = Instant::now();
        assert_eq!(timeout_ms(now, now + Duration::from_secs(1)), 0, "deadline já passou");
        assert_eq!(timeout_ms(now + Duration::from_micros(400), now), 1, "nunca 0 antes do deadline");
        assert_eq!(timeout_ms(now + Duration::from_millis(1500), now), 1500);
        assert_eq!(timeout_ms(now + Duration::from_secs(100 * 86_400), now), INFINITE - 1);
    }
}
