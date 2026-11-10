//! A janela da island.
//!
//! Estilos:
//! - `WS_POPUP`, sem bordas
//! - `WS_EX_TOPMOST | WS_EX_TOOLWINDOW` (fora da taskbar e do Alt+Tab)
//! - `WS_EX_NOACTIVATE` (clicar não rouba o foco à app onde estás)
//! - `WS_EX_NOREDIRECTIONBITMAP` (obrigatório com DirectComposition: sem bitmap GDI desperdiçado)
//!
//! Zona clicável: a janela tem o tamanho máximo expandido, recortada por uma região
//! (`SetWindowRgn`) com a forma da island (`geometry::polygon_px`). Fora da região a janela
//! não existe: não se vê e os cliques passam para as janelas de baixo. Não usar
//! `WM_NCHITTEST` -> `HTTRANSPARENT` para isto: só passa o clique a janelas da mesma thread,
//! não a outras apps. A região muda quando o **alvo** da forma muda, nunca a cada frame.
//!
//! Posição: colada ao topo do **ecrã interno do portátil**, seja qual for o portátil:
//! deteta-se pela tecnologia de ligação (`INTERNAL`, eDP/UDI embutido), não pelo nome. Sem
//! ecrã interno ativo (tampa fechada, desktop), vai para o monitor primário.
//! Reage a `WM_DPICHANGED` e `WM_DISPLAYCHANGE` (que também chega ao ligar/desligar monitores).
//!
//! O `wndproc` não toma decisões: põe o input numa fila (`take_input`) que o loop drena.
//!
//! Clique direito: menu com "Esconder" e "Fechar". Ctrl+Alt+I (atalho global) esconde e
//! mostra; escondida, a island não desenha nada.

use super::WM_APP_WAKE;
use super::geometry::{EAR, EXPANDED};
use std::cell::{Cell, RefCell};
use std::io;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreatePolygonRgn, DeleteObject, GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY,
    MONITORINFO, MonitorFromPoint, SetWindowRgn, WINDING,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, ReleaseCapture, SetCapture, TME_LEAVE,
    TRACKMOUSEEVENT, TrackMouseEvent, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_SEPARATOR, MF_STRING, PostMessageW, SW_HIDE,
    SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_HOTKEY, WM_NULL,
    CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_TOPMOST, IDC_ARROW, LoadCursorW, MA_NOACTIVATE,
    PostQuitMessage, RegisterClassExW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetWindowPos, ShowWindow,
    USER_DEFAULT_SCREEN_DPI, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_MOUSEACTIVATE,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

/// Tamanho da janela em DIPs: o painel expandido, as orelhas, e folga para o ressalto
/// das molas (`BOUNCY` passa ~6% do alvo: ~14 DIP de cada lado ao expandir).
pub const WINDOW_W: f32 = EXPANDED.w + 2.0 * EAR + 32.0;
pub const WINDOW_H: f32 = EXPANDED.h + 16.0;

const CLASS_NAME: PCWSTR = w!("DynamicIsland");
/// Está em `Win32::UI::Controls` no windows-rs: não vale a pena ativar a feature só por isto.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Retângulo em pixeis físicos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Onde está a janela (coordenadas do ecrã) e com que DPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub window: PxRect,
    pub dpi: u32,
}

impl Placement {
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32
    }
}

/// Input que o `wndproc` deixa para o loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// O rato mexeu-se sobre a janela em (`x`, `y`). O loop decide se está dentro da forma
    /// visível: a região da janela pode ser maior (a encolher ainda tem o tamanho antigo).
    Hover { x: i32, y: i32 },
    /// O rato saiu.
    Leave,
    /// A janela mudou de sítio, tamanho ou DPI.
    Moved(Placement),
    /// Botão esquerdo premido em (`x`, `y`), pixeis da janela (começa um arrastar de slider).
    Press { x: i32, y: i32 },
    /// O rato mexeu-se com o botão premido e a captura ativa (o loop pediu-a com `capture`).
    Drag { x: i32, y: i32 },
    /// Botão esquerdo largado: os botões ativam aqui, como no resto do Windows.
    Click { x: i32, y: i32 },
    /// Ficheiros a ser arrastados por cima da island (OLE, `dragdrop`).
    DragEnter,
    /// O arrastar saiu sem largar.
    DragLeave,
    /// Ficheiros largados na island (caminhos completos).
    Dropped(Vec<String>),
    /// Esconder ou voltar a mostrar a island (atalho global ou menu do clique direito).
    ToggleHidden,
    /// Uma app entrou (`true`) ou saiu (`false`) de ecrã inteiro no monitor da island.
    Fullscreen(bool),
    /// Aviso do sistema (carregador, bateria, copiar).
    System(island_core::event::SystemEvent),
}

/// Durante um arrastar, os movimentos chegam mesmo fora da janela. `false` larga.
pub fn capture(hwnd: HWND, on: bool) {
    // SAFETY: `hwnd` é a nossa janela, na thread dona.
    unsafe {
        if on {
            SetCapture(hwnd);
        } else {
            let _ = ReleaseCapture();
        }
    }
}

/// Coordenadas com sinal nas 16 palavras baixas do `lparam` (GET_X/Y_LPARAM).
fn point(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xFFFF) as u16 as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32)
}

thread_local! {
    static INPUT: RefCell<Vec<Input>> = const { RefCell::new(Vec::new()) };
    /// `TrackMouseEvent` já pedido para esta entrada do rato.
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    /// Registos de energia e clipboard, para desfazer no `WM_DESTROY`.
    static SYSTEM_REGS: RefCell<Option<super::sysevents::Registrations>> = const { RefCell::new(None) };
}

/// Input acumulado desde a última chamada. Não aloca quando não há nada.
pub fn take_input() -> Vec<Input> {
    INPUT.with_borrow_mut(std::mem::take)
}

pub fn push(input: Input) {
    INPUT.with_borrow_mut(|q| q.push(input));
}

fn px(dip: f32, dpi: u32) -> i32 {
    (dip * dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32).round() as i32
}

/// Retângulo da janela num monitor: centrada e colada ao topo. Sem chamadas ao sistema.
pub fn layout(monitor: PxRect, dpi: u32) -> PxRect {
    let (w, h) = (px(WINDOW_W, dpi), px(WINDOW_H, dpi));
    PxRect { x: monitor.x + (monitor.w - w) / 2, y: monitor.y, w, h }
}

/// Ligações de um ecrã embutido no portátil.
fn is_internal(tech: DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY) -> bool {
    [
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL,
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED,
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED,
    ]
    .contains(&tech)
}

/// Canto superior esquerdo, em pixeis do ambiente de trabalho, do ecrã interno ativo.
fn internal_display_origin() -> Option<POINT> {
    // O número de ecrãs pode mudar entre as duas chamadas (monitor ligado a meio): repetir.
    for _ in 0..3 {
        let (mut n_paths, mut n_modes) = (0, 0);
        // SAFETY: os contadores são locais e os buffers têm exatamente o tamanho declarado
        // nesses contadores; a API escreve no máximo isso e atualiza-os com o que usou.
        let (paths, modes) = unsafe {
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes) != ERROR_SUCCESS {
                return None;
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
            match QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut n_paths,
                paths.as_mut_ptr(),
                &mut n_modes,
                modes.as_mut_ptr(),
                None,
            ) {
                ERROR_SUCCESS => {}
                ERROR_INSUFFICIENT_BUFFER => continue,
                _ => return None,
            }
            paths.truncate(n_paths as usize);
            modes.truncate(n_modes as usize);
            (paths, modes)
        };
        let path = paths.iter().find(|p| is_internal(p.targetInfo.outputTechnology))?;
        // SAFETY: sem `QDC_VIRTUAL_MODE_AWARE` a union guarda `modeInfoIdx`. Um índice
        // inválido (`DISPLAYCONFIG_PATH_MODE_IDX_INVALID`) fica de fora no `get`.
        let mode = modes.get(unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize)?;
        if mode.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
            return None;
        }
        // SAFETY: `infoType` confirma que a union guarda um `sourceMode`.
        let pos = unsafe { mode.Anonymous.sourceMode.position };
        return Some(POINT { x: pos.x, y: pos.y });
    }
    None
}

/// Onde a island vive: o ecrã interno do portátil, ou o primário se não houver.
fn target_monitor() -> HMONITOR {
    // SAFETY: `MonitorFromPoint` só lê o ponto passado por valor.
    unsafe {
        internal_display_origin()
            .map(|p| MonitorFromPoint(p, MONITOR_DEFAULTTONULL))
            .filter(|m| !m.is_invalid())
            .unwrap_or_else(|| MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY))
    }
}

/// Onde a janela deve estar agora.
pub fn placement() -> io::Result<Placement> {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let (mut dpi_x, mut dpi_y) = (0, 0);
    let monitor = target_monitor();
    // SAFETY: `info`, `dpi_x` e `dpi_y` são locais e vivem até ao fim da função; `cbSize`
    // está preenchido como o `GetMonitorInfoW` exige.
    unsafe {
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return Err(io::Error::last_os_error());
        }
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)?;
    }
    let r = info.rcMonitor;
    let monitor = PxRect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top };
    Ok(Placement { window: layout(monitor, dpi_x), dpi: dpi_x })
}

/// Cria a janela **escondida**: o loop desenha o primeiro frame e aplica a região antes
/// de chamar `show`, para nunca aparecer um retângulo vazio.
pub fn create(p: Placement) -> io::Result<HWND> {
    // SAFETY: `class` vive durante o `RegisterClassExW` e as strings são literais estáticos.
    // `wndproc` tem a assinatura de `WNDPROC`. A janela é criada e usada nesta thread.
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            CLASS_NAME,
            w!("Dynamic Island"),
            WS_POPUP,
            p.window.x,
            p.window.y,
            p.window.w,
            p.window.h,
            None,
            None,
            Some(instance),
            None,
        )?)
    }
}

/// Energia (carregador, bateria) e clipboard: o Windows manda mensagens a esta janela.
pub fn register_system(hwnd: HWND) {
    SYSTEM_REGS.set(Some(super::sysevents::register(hwnd)));
}

pub fn show(hwnd: HWND) {
    set_visible(hwnd, true);
}

/// Esconder não destrói nada: a janela fica, só deixa de ser composta pelo DWM.
pub fn set_visible(hwnd: HWND, visible: bool) {
    // SAFETY: `hwnd` é a nossa janela, usada na thread dona.
    let _ = unsafe { ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
}

/// Ctrl+Alt+I em qualquer lado. Falha se outra app já usa o atalho: a island continua sem ele.
pub fn register_hotkey(hwnd: HWND) {
    // SAFETY: `hwnd` é a nossa janela; o id é nosso. Retirado no `WM_DESTROY`.
    if let Err(e) = unsafe { RegisterHotKey(Some(hwnd), HOTKEY_TOGGLE, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, u32::from(b'I')) } {
        log!("atalho Ctrl+Alt+I indisponível: {e}");
    }
}

const HOTKEY_TOGGLE: i32 = 1;
const MENU_HIDE: i32 = 1;
const MENU_CLOSE: i32 = 2;

/// Menu do clique direito no sítio do rato; devolve o comando escolhido (0 = nenhum).
fn context_menu(hwnd: HWND) -> i32 {
    // SAFETY: menu criado e destruído aqui; strings literais; `pt` é local. O
    // `SetForegroundWindow` antes e o `WM_NULL` depois são o que a documentação do
    // `TrackPopupMenu` pede para o menu fechar bem quando se clica fora.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let _ = AppendMenuW(menu, MF_STRING, MENU_HIDE as usize, w!("Esconder	Ctrl+Alt+I"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_CLOSE as usize, w!("Fechar"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0
    }
}

/// Recorta a janela a um polígono (pixeis da janela).
pub fn set_region(hwnd: HWND, points: &[(i32, i32)]) -> io::Result<()> {
    let points: Vec<POINT> = points.iter().map(|&(x, y)| POINT { x, y }).collect();
    // SAFETY: `hwnd` é a nossa janela, usada na thread dona; `points` vive durante a chamada.
    // Se `SetWindowRgn` tiver sucesso a região passa a ser do sistema e não a apagamos;
    // se falhar, continua nossa e apagamos.
    unsafe {
        let rgn = CreatePolygonRgn(&points, WINDING);
        if rgn.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        if SetWindowRgn(hwnd, Some(rgn), true) == 0 {
            let _ = DeleteObject(rgn.into());
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Volta a pôr a janela no sítio certo (o DPI ou os monitores mudaram).
fn reposition(hwnd: HWND) {
    let Ok(p) = placement() else { return };
    let r = p.window;
    // SAFETY: `hwnd` é a nossa janela e estamos na thread dona.
    let _ = unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), r.x, r.y, r.w, r.h, SWP_NOACTIVATE) };
    push(Input::Moved(p));
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Reforça o `WS_EX_NOACTIVATE`: clicar na island nunca rouba o foco.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let (x, y) = point(lparam);
            push(Input::Hover { x, y });
            // SAFETY: consulta sem argumentos.
            if unsafe { GetCapture() } == hwnd {
                push(Input::Drag { x, y });
            }
            // Pedir o `WM_MOUSELEAVE` para quando o rato sair da janela.
            if !TRACKING.get() {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: `tme` é local e está preenchido; `hwnd` é a janela desta thread.
                if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
                    TRACKING.set(true);
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            TRACKING.set(false);
            push(Input::Leave);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point(lparam);
            push(Input::Press { x, y });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = point(lparam);
            push(Input::Click { x, y });
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            match context_menu(hwnd) {
                MENU_HIDE => push(Input::ToggleHidden),
                // SAFETY: `hwnd` é a janela desta thread, a receber a sua própria mensagem.
                MENU_CLOSE => {
                    let _ = unsafe { DestroyWindow(hwnd) };
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 == HOTKEY_TOGGLE as usize => {
            push(Input::ToggleHidden);
            LRESULT(0)
        }
        WM_DESTROY => {
            // O OLE pede que o destino de arrastar seja retirado antes de a janela desaparecer.
            super::dragdrop::revoke(hwnd);
            // SAFETY: o mesmo atalho registado em `register_hotkey`, na mesma janela.
            let _ = unsafe { UnregisterHotKey(Some(hwnd), HOTKEY_TOGGLE) };
            super::fullscreen::unregister(hwnd);
            if let Some(regs) = SYSTEM_REGS.take() {
                super::sysevents::unregister(hwnd, regs);
            }
            // SAFETY: sem argumentos; põe `WM_QUIT` na fila desta thread e o loop sai.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            reposition(hwnd);
            LRESULT(0)
        }
        // O loop drena o canal depois de cada wakeup: aqui não há nada a fazer.
        WM_APP_WAKE => LRESULT(0),
        super::sysevents::WM_POWERBROADCAST => {
            super::sysevents::on_power_broadcast(wparam, lparam);
            // TRUE: o pedido de energia é aceite (o valor de retorno é ignorado nestes casos).
            LRESULT(1)
        }
        super::sysevents::WM_CLIPBOARDUPDATE => {
            super::sysevents::on_clipboard_update();
            LRESULT(0)
        }
        super::fullscreen::WM_APP_APPBAR => {
            super::fullscreen::on_message(hwnd, wparam, lparam);
            LRESULT(0)
        }
        // SAFETY: argumentos recebidos do sistema, passados tal e qual.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_HD: PxRect = PxRect { x: 0, y: 0, w: 1920, h: 1080 };

    #[test]
    fn centered_and_glued_to_the_top() {
        let r = layout(FULL_HD, 96);
        assert_eq!(r, PxRect { x: (1920 - 696) / 2, y: 0, w: 696, h: 192 });
    }

    #[test]
    fn scales_with_dpi() {
        let r = layout(FULL_HD, 120); // 125%
        assert_eq!(r, PxRect { x: (1920 - 870) / 2, y: 0, w: 870, h: 240 });
    }

    #[test]
    fn follows_monitor_origin() {
        // O ecrã interno não é necessariamente o primário: pode estar noutra origem.
        let r = layout(PxRect { x: 0, y: 1080, w: 1920, h: 1080 }, 96);
        assert_eq!((r.x, r.y), (612, 1080));
    }

    #[test]
    fn only_embedded_connections_count_as_laptop_screen() {
        use windows::Win32::Devices::Display::{
            DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EXTERNAL, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI,
        };
        assert!(is_internal(DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL));
        assert!(is_internal(DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED));
        assert!(!is_internal(DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI));
        assert!(!is_internal(DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EXTERNAL));
    }
}
