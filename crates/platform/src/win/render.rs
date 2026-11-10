//! Renderer: Direct2D desenhado numa superfície DirectComposition.
//!
//! Cadeia de criação (uma vez, e de novo se o dispositivo se perder):
//! `D3D11CreateDevice(BGRA_SUPPORT)` -> `IDXGIDevice` -> `ID2D1Device` ->
//! `DCompositionCreateDevice3` -> target para o HWND -> visual raiz -> superfície do
//! tamanho da janela (premultiplied alpha).
//!
//! Por frame: `surface.BeginDraw` -> limpar transparente -> forma do notch
//! (`geometry::outline`) a preto -> conteúdo -> `EndDraw` -> `Commit`.
//!
//! Regras de eficiência:
//! - Só se desenha quando o loop pede: a view mudou, o hover mudou ou uma mola está a mexer.
//! - O ritmo das animações é do loop (`DwmFlush`), nunca um `Sleep(16)`.
//! - O pulso ("precisa de ti") é um visual com uma animação de opacidade do **compositor**:
//!   pulsa sem acordar a app.
//! - Os layouts de texto ficam em cache e só se recriam quando a view muda.
//! - Device lost: o `ID3D11Device4` sinaliza um evento que o loop espera junto com as
//!   mensagens; erros de `BeginDraw`/`EndDraw`/`Commit` também deitam a cadeia fora.
//!   A cadeia é recriada no frame seguinte.

use super::backdrop;
use super::geometry::{self, ARC_STEPS, EAR, Mode, PAD, REST, Size};
use super::panel::{Hit, Image, PanelContent, Tab, draw_image};
use super::window::Placement;
use island_core::anim::{IslandShape, Spring, SpringParams};
use island_core::event::ImageData;
use island_core::{Icon, ModuleId, Presented, Priority, Widget};
use windows::Win32::Foundation::{CloseHandle, E_FAIL, HANDLE, HMODULE, HWND, POINT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_COLOR_F, D2D1_GRADIENT_STOP, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED,
    D2D1_FIGURE_END_OPEN, D2D1_FILL_MODE_WINDING,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_BUFFER_PRECISION_8BPC_UNORM, D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED, D2D1_ANTIALIAS_MODE_ALIASED, D2D1_COLOR_SPACE_SRGB,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, ID2D1LinearGradientBrush,
    D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Brush, ID2D1DeviceContext,
    ID2D1BitmapBrush1, ID2D1Factory1, ID2D1PathGeometry, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device, ID3D11Device4,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice3, IDCompositionDesktopDevice, IDCompositionSurface, IDCompositionTarget,
    IDCompositionVisual, IDCompositionVisual2, IDCompositionVisual3,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory, WICBitmapCacheOnLoad,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeHighQualityCubic, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, DXGI_GPU_PREFERENCE_MINIMUM_POWER,
    IDXGIAdapter, IDXGIDevice, IDXGIFactory6,
};
use windows::Win32::Foundation::D2DERR_RECREATE_TARGET;
use windows::Win32::System::Threading::CreateEventW;
use windows::core::{BOOL, Error, HSTRING, Interface, Result};
use windows_numerics::{Matrix3x2, Vector2};

/// Caixa do ícone, espaço entre elementos e ponto de pulso (DIP).
const ICON_BOX: f32 = 20.0;
const GAP: f32 = 8.0;
const DOT: f32 = 8.0;
/// Barra de volume (HUD) quando não há texto.
const BAR_W: f32 = 120.0;
const BAR_H: f32 = 4.0;
const ORANGE: D2D1_COLOR_F = D2D1_COLOR_F { r: 1.0, g: 0.624, b: 0.039, a: 1.0 };
const WHITE: D2D1_COLOR_F = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
/// Opacidade do preto quando a island cresce (vidro). Em repouso é 1,0: preto puro, como o notch.
const GLASS_ALPHA: f32 = 0.72;
/// Com o vidro desligado (config) ou os efeitos de transparência do Windows desligados.
const SOLID_ALPHA: f32 = 0.96;
/// Preto sobre o fundo desfocado (vidro com fotografia): deixa ver as cores por trás.
const TINT_ALPHA: f32 = 0.40;
/// Duração da transição entre dois conteúdos do notch.
const FADE_SECS: f32 = 0.18;
/// Altura (DIP) do brilho no topo do vidro.
const HIGHLIGHT_H: f32 = 24.0;
const BLACK: D2D1_COLOR_F = D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };

/// Resultado de um frame, para o loop.
#[derive(Debug)]
pub struct FrameOut {
    /// Uma mola ainda se mexe: o loop pede outro frame.
    pub animating: bool,
    /// Nova região da janela (pixeis da janela), quando o alvo mudou ou a animação acabou.
    pub region: Option<Vec<(i32, i32)>>,
    /// A cadeia gráfica não pôde ser criada (driver a reiniciar): tentar outra vez daqui a pouco.
    pub retry: bool,
}

/// Recursos que não dependem da GPU: sobrevivem a um device lost.
///
/// Os formatos são de uma linha, cortada com reticências. Os do painel centram o texto na
/// vertical, para se desenhar no canto do retângulo do `panel_layout`. Testável sem GPU.
#[derive(Debug)]
pub(super) struct TextKit {
    dwrite: IDWriteFactory,
    /// Glifos (16), centrados nas duas direções.
    pub(super) icon: IDWriteTextFormat,
    /// Glifo grande da capa vazia (28).
    pub(super) icon_large: IDWriteTextFormat,
    /// Texto do notch compacto (13 semibold).
    compact: IDWriteTextFormat,
    /// Títulos do painel (13 semibold).
    pub(super) strong: IDWriteTextFormat,
    /// Texto corrido (12).
    pub(super) text: IDWriteTextFormat,
    /// Nome do projeto nas sessões (12 semibold).
    pub(super) label: IDWriteTextFormat,
    /// Glifos minúsculos (9), centrados: o ✕ da Tray.
    pub(super) icon_tiny: IDWriteTextFormat,
    /// Glifos dos botões pequenos (12), centrados.
    pub(super) icon_small: IDWriteTextFormat,
    /// Etiquetas pequenas (11 semibold).
    pub(super) caption: IDWriteTextFormat,
    /// Linhas secundárias (11).
    pub(super) caption_plain: IDWriteTextFormat,
    /// Aviso centrado (11).
    pub(super) caption_center: IDWriteTextFormat,
    /// Separadores (12 semibold), centrados.
    pub(super) tab: IDWriteTextFormat,
}

/// Tudo o que vive na GPU. Deita-se fora inteiro quando o dispositivo se perde.
#[derive(Debug)]
struct Gpu {
    d3d: ID3D11Device,
    removed: Option<(HANDLE, u32)>,
    dcomp: IDCompositionDesktopDevice,
    _target: IDCompositionTarget,
    root: IDCompositionVisual2,
    surface: IDCompositionSurface,
    pulse: IDCompositionVisual3,
    pulse_on: bool,
    white: ID2D1SolidColorBrush,
    black: ID2D1SolidColorBrush,
    /// Pincel de cor variável (`SetColor`) para o painel.
    paint: ID2D1SolidColorBrush,
    /// Brilho no topo do vidro: branco a 10% que desvanece em ~24 DIP.
    highlight: ID2D1LinearGradientBrush,
    /// Rebordo: branco forte em cima, quase nada em baixo.
    rim: ID2D1LinearGradientBrush,
    /// Contexto só para criar recursos fora do `BeginDraw` (a capa).
    res: ID2D1DeviceContext,
    /// Capa já na GPU, criada a partir de `Art::bitmap` quando falta.
    art: Option<ID2D1BitmapBrush1>,
    /// Ícone da app (badge sobre a capa).
    badge: Option<ID2D1BitmapBrush1>,
    /// Ícone da app no toast de notificação.
    toast_icon: Option<ID2D1BitmapBrush1>,
    /// Fotografia desfocada do ecrã atrás da island (`backdrop`), tirada ao sair do repouso.
    backdrop: Option<ID2D1BitmapBrush1>,
    /// Ícones da Tray, alinhados com `Renderer::tray`.
    tray: Vec<Option<ID2D1BitmapBrush1>>,
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some((event, cookie)) = self.removed.take() {
            // SAFETY: o cookie veio do `RegisterDeviceRemovedEvent` deste dispositivo e o
            // evento foi criado por nós; nenhum dos dois volta a ser usado.
            unsafe {
                if let Ok(d4) = self.d3d.cast::<ID3D11Device4>() {
                    d4.UnregisterDeviceRemoved(cookie);
                }
                let _ = CloseHandle(event);
            }
        }
    }
}

/// O que se desenha para a view atual. Recriado só quando a view muda.
#[derive(Debug, Default)]
struct Content {
    view: Option<Presented>,
    /// Prioridade `Info` ou mais e algo para mostrar. Abaixo disso o notch fica só preto.
    compact: bool,
    icon: Option<IDWriteTextLayout>,
    text: Option<(IDWriteTextLayout, f32, f32)>,
    progress: Option<f32>,
    pulse: bool,
    /// Notificação de uma app: textos já em layouts.
    toast: Option<ToastContent>,
    /// Largura do conteúdo compacto, para `Mode::Event`.
    compact_w: f32,
}

/// Textos do toast de uma notificação: título, nome da app e uma linha de corpo.
#[derive(Debug)]
struct ToastContent {
    title: (IDWriteTextLayout, f32, f32),
    app: Option<(IDWriteTextLayout, f32)>,
    body: Option<(IDWriteTextLayout, f32)>,
}

/// Largura disponível para o texto do toast: sem as margens, o ícone e o espaço entre ambos.
const TOAST_TEXT_W: f32 = geometry::TOAST.w - 2.0 * geometry::TOAST_PAD - geometry::TOAST_ICON - 12.0;

#[derive(Debug)]
pub struct Renderer {
    hwnd: HWND,
    placement: Placement,
    d2d: ID2D1Factory1,
    text: TextKit,
    gpu: Option<Gpu>,
    shape: IslandShape,
    target: Size,
    /// Falta mandar a região final (no fim da animação ou depois de recriar a janela).
    region_pending: bool,
    expanded: bool,
    content: Content,
    panel: PanelContent,
    /// O painel está aberto e assente: só então os cliques contam.
    panel_ready: bool,
    art: Art,
    badge: Art,
    toast_icon: Art,
    /// Conteúdo anterior, a desvanecer enquanto o novo aparece.
    prev: Option<Content>,
    /// Progresso da transição de conteúdo (1 = acabou).
    fade: f32,
    tray: Vec<Art>,
    drop_hover: bool,
    /// Vidro translúcido (config `vidro`); desligado, fica preto quase opaco.
    glass: bool,
}

impl Renderer {
    /// Liga ou desliga o vidro. O próximo frame redesenha.
    pub fn set_glass(&mut self, on: bool) {
        self.glass = on;
    }

    pub fn new(hwnd: HWND, placement: Placement) -> Result<Self> {
        let rest = geometry::REST;
        let mut shape = IslandShape::new(rest.w, rest.h);
        shape.radius = Spring::new(rest.r, SpringParams::BOUNCY);
        // SAFETY: fábricas sem argumentos de ponteiro.
        let d2d: ID2D1Factory1 = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let mut r = Self {
            hwnd,
            placement,
            d2d,
            text: TextKit::new()?,
            gpu: None,
            shape,
            target: rest,
            region_pending: true,
            expanded: false,
            content: Content::default(),
            panel: PanelContent::default(),
            panel_ready: false,
            art: Art::default(),
            badge: Art::default(),
            toast_icon: Art::default(),
            prev: None,
            fade: 1.0,
            tray: Vec::new(),
            drop_hover: false,
            glass: true,
        };
        r.gpu = Some(Gpu::new(hwnd, &r.d2d, placement)?);
        Ok(r)
    }

    /// Evento sinalizado quando o dispositivo gráfico se perde (o loop espera por ele).
    pub fn lost_event(&self) -> Option<HANDLE> {
        self.gpu.as_ref().and_then(|g| g.removed.map(|(event, _)| event))
    }

    /// O dispositivo perdeu-se: largar a cadeia; o próximo frame recria-a.
    pub fn device_lost(&mut self) {
        self.gpu = None;
        self.region_pending = true;
    }

    /// A janela mudou de tamanho ou DPI: a superfície tem de acompanhar.
    pub fn moved(&mut self, placement: Placement) {
        self.placement = placement;
        // A capa foi reduzida para o DPI antigo: descodificar outra vez.
        self.art = Art::default();
        self.badge = Art::default();
        self.toast_icon = Art::default();
        self.tray.clear();
        self.device_lost();
    }

    /// Descodifica a capa nova, se a música mudou. Uma vez por capa, não por frame.
    fn update_art(&mut self, panel: &[(ModuleId, Widget)]) {
        let media = panel.iter().find_map(|(_, w)| match w {
            Widget::Media(m) => Some(m),
            _ => None,
        });
        let scale = self.placement.scale();
        if self.art.update(media.and_then(|m| m.artwork.as_ref()), geometry::ART * scale)
            && let Some(gpu) = &mut self.gpu
        {
            gpu.art = None;
        }
        if self.badge.update(media.and_then(|m| m.app_icon.as_ref()), geometry::BADGE * scale)
            && let Some(gpu) = &mut self.gpu
        {
            gpu.badge = None;
        }
        // Ícones da Tray: um `Art` por ficheiro, pela ordem da lista. Só se refaz quando a
        // lista de ícones muda (no máximo 7, de 48 DIP).
        let icons: Vec<Option<&ImageData>> = panel
            .iter()
            .find_map(|(_, w)| match w {
                Widget::Tray(t) => Some(t.items.iter().map(|i| i.icon.as_ref()).collect()),
                _ => None,
            })
            .unwrap_or_default();
        let current: Vec<Option<&ImageData>> = self.tray.iter().map(|a| a.key.as_ref()).collect();
        if icons != current {
            self.tray = icons
                .iter()
                .map(|icon| {
                    let mut art = Art::default();
                    art.update(*icon, geometry::TRAY_ICON * scale);
                    art
                })
                .collect();
            if let Some(gpu) = &mut self.gpu {
                gpu.tray = vec![None; self.tray.len()];
            }
        }
    }

    /// Avança as molas `dt` segundos e desenha. `current` é o compacto (o vencedor),
    /// `panel` os blocos de todos os módulos para o painel expandido. `drop_hover`: estão a
    /// arrastar ficheiros por cima (realça a zona de largar da Tray).
    pub fn frame(
        &mut self,
        dt: f32,
        current: Option<&Presented>,
        panel: &[(ModuleId, Widget)],
        expanded: bool,
        tab: Tab,
        drop_hover: bool,
    ) -> FrameOut {
        self.drop_hover = drop_hover;
        if self.content.view.as_ref() != current {
            let new = self.text.content(current).unwrap_or_else(|e| {
                log!("render: texto: {e}");
                Content { view: current.cloned(), ..Content::default() }
            });
            // Transição cruzada: o conteúdo antigo desvanece enquanto o novo aparece.
            if crossfades(&self.content, &new) {
                self.prev = Some(std::mem::replace(&mut self.content, new));
                self.fade = 0.0;
            } else {
                self.content = new;
            }
        }
        self.update_art(panel);
        if expanded && let Err(e) = self.panel.update(&self.text, panel, tab) {
            log!("render: painel: {e}");
        }
        self.expanded = expanded;
        // O ícone do toast (muda com a notificação, não com o frame).
        let toast_icon = self.content.view.as_ref().and_then(|p| p.view.compact.toast.as_ref()).and_then(|t| t.icon.as_ref());
        if self.toast_icon.update(toast_icon, geometry::TOAST_ICON * self.placement.scale())
            && let Some(gpu) = &mut self.gpu
        {
            gpu.toast_icon = None;
        }
        let mode = if expanded {
            Mode::Expanded
        } else if self.content.toast.is_some() {
            Mode::Toast
        } else if self.content.compact {
            Mode::Event { content_w: self.content.compact_w }
        } else {
            Mode::Rest
        };
        let target = geometry::target(mode);
        let mut region = None;
        if target != self.target {
            // Ao sair do repouso, fotografar o que está atrás (a pílula ainda está no sítio).
            if self.glass && target != REST && self.at_rest() {
                self.snapshot_backdrop();
            }
            // A crescer, a região já tem o tamanho novo; a encolher, mantém o atual até ao fim.
            region = Some(self.region(self.size().envelope(target)));
            self.target = target;
            self.shape.set_target(target.w, target.h, target.r);
            self.region_pending = true;
        }
        let shape_animating = self.shape.step(dt);
        if !shape_animating && self.region_pending {
            self.region_pending = false;
            region = Some(self.region(target));
        }
        // De volta ao repouso: a fotografia já não serve (e liberta a memória).
        if !shape_animating
            && target == REST
            && let Some(gpu) = &mut self.gpu
        {
            gpu.backdrop = None;
        }
        // Transição entre conteúdos.
        let mut fading = false;
        if self.expanded {
            self.prev = None;
            self.fade = 1.0;
        } else if self.fade < 1.0 {
            self.fade = (self.fade + dt / FADE_SECS).min(1.0);
            if self.fade >= 1.0 {
                self.prev = None;
            } else {
                fading = true;
            }
        }
        let animating = shape_animating || fading;

        let mut retry = false;
        if let Err(e) = self.draw() {
            if is_device_lost(&e) || self.gpu.is_none() {
                self.device_lost();
                retry = true;
            } else {
                log!("render: {e}");
            }
        }
        FrameOut { animating, region, retry }
    }

    /// A forma está em repouso (a pílula preta no sítio)?
    fn at_rest(&self) -> bool {
        (self.shape.width.value - REST.w).abs() < 6.0 && (self.shape.height.value - REST.h).abs() < 4.0
    }

    /// Fotografa e desfoca o ecrã atrás da janela (ver `backdrop`). Se falhar, o vidro fica
    /// só translúcido, sem desfoque.
    fn snapshot_backdrop(&mut self) {
        let Some(gpu) = &mut self.gpu else { return };
        // SAFETY: contexto de recursos desta cadeia, na thread que o criou.
        let made = backdrop::capture(&gpu.res, self.placement.window, self.placement.dpi)
            .and_then(|bitmap| unsafe { gpu.res.CreateBitmapBrush(&bitmap, None, None) });
        match made {
            Ok(brush) => gpu.backdrop = Some(brush),
            Err(e) => log!("vidro: {e}"),
        }
    }

    fn size(&self) -> Size {
        Size { w: self.shape.width.value, h: self.shape.height.value, r: self.shape.radius.value }
    }

    fn region(&self, s: Size) -> Vec<(i32, i32)> {
        geometry::polygon_px(geometry::with_glow(s), self.placement.scale(), self.placement.window.w / 2)
    }

    fn draw(&mut self) -> Result<()> {
        if self.gpu.is_none() {
            self.gpu = Some(Gpu::new(self.hwnd, &self.d2d, self.placement)?);
        }
        let Some(gpu) = self.gpu.as_mut() else { return Err(Error::from_hresult(E_FAIL)) };
        let scale = self.placement.scale();
        let dpi = self.placement.dpi as f32;
        let center = self.placement.window.w as f32 / scale / 2.0;
        let s = Size { w: self.shape.width.value, h: self.shape.height.value, r: self.shape.radius.value };
        let geometry = path(&self.d2d, s, true)?;
        let edge = path(&self.d2d, s, false)?;
        // O conteúdo aparece à medida que a forma chega ao alvo (sem frames extra).
        let distance = (s.w - self.target.w).abs() + (s.h - self.target.h).abs();
        let alpha = (1.0 - distance / 40.0).clamp(0.0, 1.0);
        self.panel_ready = self.expanded && alpha > 0.9;

        let mut offset = POINT::default();
        // SAFETY: `offset` é local; o contexto devolvido só é usado entre este `BeginDraw` e
        // o `EndDraw` a seguir, que é sempre chamado.
        let dc: ID2D1DeviceContext = unsafe { gpu.surface.BeginDraw(None, &mut offset)? };
        // SAFETY: chamadas de desenho no contexto válido do `BeginDraw`, com argumentos locais.
        let drawn = unsafe {
            dc.SetDpi(dpi, dpi);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.SetTransform(&Matrix3x2::translation(offset.x as f32 / scale + center, offset.y as f32 / scale));
            dc.Clear(Some(&D2D1_COLOR_F::default()));
            // Vidro: em repouso, preto puro; quanto mais a forma cresce, mais translúcido
            // (deixa ver o que está por trás) e mais se nota o brilho e o rebordo.
            let grown = (((s.w - REST.w).abs() + (s.h - REST.h).abs()) / 24.0).clamp(0.0, 1.0);
            let top = if self.glass { GLASS_ALPHA } else { SOLID_ALPHA };
            if let Some(photo) = gpu.backdrop.as_ref().filter(|_| self.glass && grown > 0.0) {
                // Fundo desfocado por baixo e uma tinta escura por cima (em repouso, preto puro).
                photo.SetTransform(&Matrix3x2::translation(-center, 0.0));
                dc.FillGeometry(&geometry, photo, None::<&ID2D1Brush>);
                gpu.black.SetOpacity(1.0 - (1.0 - TINT_ALPHA) * grown);
            } else {
                gpu.black.SetOpacity(1.0 - (1.0 - top) * grown);
            }
            dc.FillGeometry(&geometry, &gpu.black, None::<&ID2D1Brush>);
            if self.glass {
                gpu.highlight.SetOpacity(grown);
                // Só a faixa do topo: pintar a forma toda gastava ~40% mais por frame (WARP).
                let strip = D2D_RECT_F { left: -s.w / 2.0 - EAR, top: 0.0, right: s.w / 2.0 + EAR, bottom: HIGHLIGHT_H.min(s.h) };
                dc.PushAxisAlignedClip(&strip, D2D1_ANTIALIAS_MODE_ALIASED);
                dc.FillGeometry(&geometry, &gpu.highlight, None::<&ID2D1Brush>);
                dc.PopAxisAlignedClip();
            }
            // Rebordo com gradiente ao longo da altura da forma: linha de 1 DIP (nas orelhas
            // também), sem a aresta colada ao topo do ecrã.
            gpu.rim.SetEndPoint(Vector2 { X: 0.0, Y: s.h.max(1.0) });
            gpu.rim.SetOpacity(if self.glass { grown } else { grown * 0.5 });
            dc.DrawGeometry(&edge, &gpu.rim, 1.0, None::<&ID2D1StrokeStyle>);
            // Durante quase toda a transição o conteúdo está invisível: não o desenhar
            // (com WARP, o texto é a parte cara de cada frame).
            let fade_in = self.fade * self.fade * (3.0 - 2.0 * self.fade); // smoothstep
            let result: Result<()> = if alpha < 0.01 {
                Ok(())
            } else if self.expanded {
                let art = self.art.brush(&mut gpu.art, &gpu.res);
                let badge = self.badge.brush(&mut gpu.badge, &gpu.res);
                gpu.tray.resize(self.tray.len(), None);
                let tray: Vec<_> =
                    self.tray.iter().zip(gpu.tray.iter_mut()).map(|(art, slot)| art.brush(slot, &gpu.res)).collect();
                self.panel.draw(&dc, &gpu.paint, art, badge, &tray, self.drop_hover, alpha);
                Ok(())
            } else if let Some(toast) = &self.content.toast {
                let icon = self.toast_icon.brush(&mut gpu.toast_icon, &gpu.res);
                draw_toast(&dc, &gpu.white, &gpu.paint, toast, &self.content, icon, s, alpha * fade_in);
                Ok(())
            } else if self.content.compact {
                draw_compact(&dc, &gpu.white, &self.content, s, alpha * fade_in)
            } else {
                Ok(())
            };
            // O conteúdo antigo desvanece por cima/ao lado do novo, recortado à forma atual.
            if !self.expanded
                && let Some(prev) = &self.prev
            {
                let a = 1.0 - fade_in;
                let clip = D2D_RECT_F { left: -s.w / 2.0, top: 0.0, right: s.w / 2.0, bottom: s.h };
                dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
                if let Some(t) = &prev.toast {
                    draw_toast(&dc, &gpu.white, &gpu.paint, t, prev, None, s, a);
                } else if prev.compact {
                    let _ = draw_compact(&dc, &gpu.white, prev, s, a);
                }
                dc.PopAxisAlignedClip();
            }
            result
        };
        // SAFETY: fecha o `BeginDraw` acima, mesmo que o desenho tenha falhado.
        unsafe { gpu.surface.EndDraw()? };
        drawn?;

        let pulse = self.content.pulse && !self.expanded && alpha > 0.5;
        let dot = (
            self.placement.window.w as f32 / 2.0 + (s.w / 2.0 - PAD - DOT) * scale,
            (s.h / 2.0 - DOT / 2.0) * scale,
        );
        gpu.set_pulse(pulse, dot)?;
        // SAFETY: sem argumentos; publica as alterações deste frame no compositor.
        unsafe { gpu.dcomp.Commit() }
    }

    /// O que está debaixo de um clique em (`x`, `y`), pixeis da janela. Só o painel aberto
    /// tem controlos; um clique no notch compacto não faz nada (o hover já o abre).
    pub fn hit_test(&self, x: i32, y: i32) -> Option<Hit> {
        // O toast de uma notificação: clicar abre a app.
        if !self.expanded
            && let Some(p) = &self.content.view
            && let Some(t) = &p.view.compact.toast
            && self.content.toast.is_some()
            && self.shape_contains(x, y)
        {
            return Some(Hit::Control(p.module, t.open));
        }
        if !self.panel_ready {
            return None;
        }
        let (px, py) = self.panel_point(x, y);
        self.panel.hit_test(px, py)
    }

    /// Há um toast de notificação à mostra? Enquanto houver, passar o rato por cima não abre o
    /// painel: o clique tem de chegar ao toast (abre a app).
    pub fn toast_showing(&self) -> bool {
        self.content.toast.is_some()
    }

    /// O ponto (pixeis da janela) está dentro da forma tal como está desenhada agora?
    pub fn shape_contains(&self, x: i32, y: i32) -> bool {
        let (px, py) = self.panel_point(x, y);
        geometry::contains(self.size(), px, py)
    }

    /// Pixeis da janela -> DIPs na origem do contorno (centro da borda de cima).
    pub fn panel_point(&self, x: i32, y: i32) -> (f32, f32) {
        let scale = self.placement.scale();
        ((x - self.placement.window.w / 2) as f32 / scale, y as f32 / scale)
    }
}

/// Capa da música, descodificada e já reduzida ao tamanho de ecrã. Não depende da GPU
/// (sobrevive a um device lost); a versão na GPU é `Gpu::art`.
#[derive(Debug, Default)]
struct Art {
    key: Option<ImageData>,
    /// Bitmap em PBGRA e o seu tamanho em pixeis (o lado menor = capa em pixeis).
    bitmap: Option<(IWICBitmap, (u32, u32))>,
}

impl Art {
    /// Descodifica `image` se mudou, já com o lado menor = `side_dip` em pixeis.
    /// Devolve `true` se mudou (a versão na GPU tem de ser recriada).
    fn update(&mut self, image: Option<&ImageData>, side_px: f32) -> bool {
        if self.key.as_ref() == image {
            return false;
        }
        self.key = image.cloned();
        self.bitmap = image.and_then(|a| {
            decode_art(&a.0, side_px.round() as u32).map_err(|e| log!("render: imagem inválida: {e}")).ok()
        });
        true
    }

    /// O pincel na GPU (criado se falta) e o tamanho do bitmap.
    fn brush<'a>(
        &self,
        slot: &'a mut Option<ID2D1BitmapBrush1>,
        res: &ID2D1DeviceContext,
    ) -> Option<(&'a ID2D1BitmapBrush1, (u32, u32))> {
        let (bitmap, size) = self.bitmap.as_ref()?;
        if slot.is_none() {
            *slot = art_brush(res, bitmap).map_err(|e| log!("render: imagem: {e}")).ok();
        }
        slot.as_ref().map(|b| (b, *size))
    }
}

/// PNG/JPEG -> bitmap PBGRA com o lado menor igual a `side` pixeis. Reduzir logo aqui
/// guarda ~32 KB em vez da imagem inteira (as capas chegam a 1000×1000).
fn decode_art(bytes: &[u8], side: u32) -> Result<(IWICBitmap, (u32, u32))> {
    // SAFETY: COM está inicializado nesta thread (STA da UI). O stream lê de `bytes`, que
    // vive até ao fim da função; `CreateBitmapFromSource(CacheOnLoad)` copia os pixeis,
    // por isso o bitmap devolvido já não depende de `bytes`.
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let (mut w, mut h) = (0, 0);
        frame.GetSize(&mut w, &mut h)?;
        let k = side.max(1) as f32 / w.min(h).max(1) as f32;
        let (tw, th) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&frame, tw, th, WICBitmapInterpolationModeHighQualityCubic)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        Ok((wic.CreateBitmapFromSource(&converter, WICBitmapCacheOnLoad)?, (tw, th)))
    }
}

/// A capa como pincel, para a desenhar com cantos arredondados num só `FillRoundedRectangle`.
fn art_brush(res: &ID2D1DeviceContext, bitmap: &IWICBitmap) -> Result<ID2D1BitmapBrush1> {
    // SAFETY: contexto de recursos desta cadeia, usado na thread que o criou.
    unsafe {
        let bitmap = res.CreateBitmapFromWicBitmap(bitmap, None)?;
        res.CreateBitmapBrush(&bitmap, None, None)
    }
}

/// Os dois conteúdos são "coisas diferentes" (módulo, texto ou notificação): faz-se a transição
/// cruzada. Só o progresso a mexer (arrastar o volume) não conta: seria um pisca-pisca.
fn crossfades(old: &Content, new: &Content) -> bool {
    if !old.compact {
        return false;
    }
    match (&old.view, &new.view) {
        (Some(a), Some(b)) => {
            a.module != b.module
                || a.view.compact.text != b.view.compact.text
                || a.view.compact.toast != b.view.compact.toast
        }
        _ => true,
    }
}

fn is_device_lost(e: &Error) -> bool {
    [DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, D2DERR_RECREATE_TARGET].contains(&e.code())
}

/// Path do D2D a partir do mesmo contorno que a região da janela. `closed: false` deixa de
/// fora o segmento ao longo da borda de cima do ecrã (para a borda azul não o desenhar).
fn path(factory: &ID2D1Factory1, s: Size, closed: bool) -> Result<ID2D1PathGeometry> {
    let pts: Vec<Vector2> = geometry::outline(s, ARC_STEPS).into_iter().map(|(x, y)| Vector2 { X: x, Y: y }).collect();
    // SAFETY: `pts` vive durante as chamadas; o sink é fechado antes de a geometria ser usada.
    unsafe {
        let geom: ID2D1PathGeometry = factory.CreatePathGeometry()?.cast()?;
        let sink = geom.Open()?;
        sink.SetFillMode(D2D1_FILL_MODE_WINDING);
        sink.BeginFigure(pts[0], if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
        sink.AddLines(&pts[1..]);
        sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
        sink.Close()?;
        Ok(geom)
    }
}

/// Notificação de uma app: ícone à esquerda; à direita o título (com o nome da app ao lado,
/// se couber) e, por baixo, uma linha de texto.
///
/// # Safety
/// `dc` tem de estar entre `BeginDraw` e `EndDraw`.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_toast(
    dc: &ID2D1DeviceContext,
    white: &ID2D1SolidColorBrush,
    paint: &ID2D1SolidColorBrush,
    t: &ToastContent,
    c: &Content,
    icon: Image,
    s: Size,
    alpha: f32,
) {
    let left = -s.w / 2.0 + geometry::TOAST_PAD;
    let icon_rect = geometry::Rect { x: left, y: (s.h - geometry::TOAST_ICON) / 2.0, w: geometry::TOAST_ICON, h: geometry::TOAST_ICON };
    // SAFETY: garantido por quem chama (contexto dentro de `BeginDraw`).
    unsafe {
        match icon {
            Some(image) => draw_image(dc, image, &icon_rect, 10.0, alpha),
            None => {
                // Sem ícone: um quadrado de vidro com o glifo da view.
                paint.SetColor(&WHITE);
                paint.SetOpacity(alpha * 0.14);
                let rect = D2D_RECT_F { left: icon_rect.x, top: icon_rect.y, right: icon_rect.x + icon_rect.w, bottom: icon_rect.y + icon_rect.h };
                dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: 10.0, radiusY: 10.0 }, paint);
                if let Some(glyph) = &c.icon {
                    white.SetOpacity(alpha);
                    let o = (icon_rect.w - ICON_BOX) / 2.0;
                    dc.DrawTextLayout(Vector2 { X: icon_rect.x + o, Y: icon_rect.y + o }, glyph, white, D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
            }
        }
        let x = icon_rect.x + icon_rect.w + 12.0;
        let (title, title_w, title_h) = &t.title;
        let body_h = if t.body.is_some() { 17.0 } else { 0.0 };
        let top = (s.h - (title_h + body_h)) / 2.0;
        white.SetOpacity(alpha);
        dc.DrawTextLayout(Vector2 { X: x, Y: top }, title, white, D2D1_DRAW_TEXT_OPTIONS_NONE);
        if let Some((app, app_w)) = &t.app
            && title_w + 8.0 + app_w <= TOAST_TEXT_W
        {
            white.SetOpacity(alpha * 0.45);
            dc.DrawTextLayout(Vector2 { X: x + title_w + 8.0, Y: top + 1.0 }, app, white, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        if let Some((body, _)) = &t.body {
            white.SetOpacity(alpha * 0.62);
            dc.DrawTextLayout(Vector2 { X: x, Y: top + title_h }, body, white, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }
}

/// Ícone, texto e barra de progresso, centrados na altura do notch.
///
/// # Safety
/// `dc` tem de estar entre `BeginDraw` e `EndDraw`.
unsafe fn draw_compact(
    dc: &ID2D1DeviceContext,
    white: &ID2D1SolidColorBrush,
    c: &Content,
    s: Size,
    alpha: f32,
) -> Result<()> {
    let left = -s.w / 2.0 + PAD;
    let right = s.w / 2.0 - PAD - if c.pulse { DOT + GAP } else { 0.0 };
    // SAFETY: garantido por quem chama (contexto dentro de `BeginDraw`).
    unsafe {
        white.SetOpacity(alpha);
        if let Some(icon) = &c.icon {
            dc.DrawTextLayout(Vector2 { X: left, Y: (s.h - ICON_BOX) / 2.0 }, icon, white, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        let x = left + ICON_BOX + GAP;
        if let Some((text, _, h)) = &c.text {
            dc.DrawTextLayout(Vector2 { X: x, Y: (s.h - h) / 2.0 }, text, white, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        if let Some(p) = c.progress {
            // Com texto, uma linha fina em baixo; sem texto (HUD de volume), uma barra ao lado do ícone.
            let (x0, x1, y) = match &c.text {
                Some(_) => (x, right, s.h - 7.0),
                None => (x, (x + BAR_W).min(right), (s.h - BAR_H) / 2.0),
            };
            let bar = |x1: f32| D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: x0, top: y, right: x1, bottom: y + BAR_H },
                radiusX: BAR_H / 2.0,
                radiusY: BAR_H / 2.0,
            };
            white.SetOpacity(alpha * 0.2);
            dc.FillRoundedRectangle(&bar(x1), white);
            white.SetOpacity(alpha * 0.9);
            dc.FillRoundedRectangle(&bar(x0 + (x1 - x0) * p.clamp(0.0, 1.0)), white);
        }
    }
    Ok(())
}

impl Gpu {
    fn new(hwnd: HWND, factory: &ID2D1Factory1, p: Placement) -> Result<Self> {
        let d3d = create_d3d()?;
        let scale = p.scale();
        // SAFETY: interfaces COM criadas aqui e usadas nesta thread; os argumentos de ponteiro
        // são locais. O target é o único deste HWND (o anterior já foi largado).
        unsafe {
            let dxgi: IDXGIDevice = d3d.cast()?;
            let d2d_device = factory.CreateDevice(&dxgi)?;
            let dc = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let white = dc.CreateSolidColorBrush(&WHITE, None)?;
            let black = dc.CreateSolidColorBrush(&BLACK, None)?;
            let paint = dc.CreateSolidColorBrush(&WHITE, None)?;
            let gradient = |stops: &[(f32, f32)], end: f32| -> Result<ID2D1LinearGradientBrush> {
                let stops: Vec<D2D1_GRADIENT_STOP> = stops
                    .iter()
                    .map(|&(position, a)| D2D1_GRADIENT_STOP { position, color: D2D1_COLOR_F { a, ..WHITE } })
                    .collect();
                let collection = dc.CreateGradientStopCollection(
                    &stops,
                    D2D1_COLOR_SPACE_SRGB,
                    D2D1_COLOR_SPACE_SRGB,
                    D2D1_BUFFER_PRECISION_8BPC_UNORM,
                    D2D1_EXTEND_MODE_CLAMP,
                    D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED,
                )?;
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: Vector2 { X: 0.0, Y: 0.0 },
                    endPoint: Vector2 { X: 0.0, Y: end },
                };
                dc.CreateLinearGradientBrush(&props, None, &collection)
            };
            let highlight = gradient(&[(0.0, 0.10), (1.0, 0.0)], HIGHLIGHT_H)?;
            let rim = gradient(&[(0.0, 0.46), (0.75, 0.06), (1.0, 0.06)], 1.0)?;

            let dcomp: IDCompositionDesktopDevice = DCompositionCreateDevice3(&d2d_device)?;
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let root = dcomp.CreateVisual()?;
            let (w, h) = (p.window.w.max(1) as u32, p.window.h.max(1) as u32);
            let surface = dcomp.CreateSurface(w, h, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            root.SetContent(&surface)?;
            target.SetRoot(&root)?;

            // Pulso: um ponto laranja desenhado uma vez; a opacidade é animada pelo compositor.
            let side = (DOT * scale).ceil() as u32 + 2;
            let dot_surface = dcomp.CreateSurface(side, side, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            let mut offset = POINT::default();
            let ddc: ID2D1DeviceContext = dot_surface.BeginDraw(None, &mut offset)?;
            ddc.SetDpi(p.dpi as f32, p.dpi as f32);
            ddc.SetTransform(&Matrix3x2::translation(offset.x as f32 / scale, offset.y as f32 / scale));
            ddc.Clear(Some(&D2D1_COLOR_F::default()));
            let orange = ddc.CreateSolidColorBrush(&ORANGE, None)?;
            let c = side as f32 / scale / 2.0;
            ddc.FillEllipse(
                &D2D1_ELLIPSE { point: Vector2 { X: c, Y: c }, radiusX: DOT / 2.0, radiusY: DOT / 2.0 },
                &orange,
            );
            dot_surface.EndDraw()?;
            let pulse: IDCompositionVisual3 = dcomp.CreateVisual()?.cast()?;
            pulse.SetContent(&dot_surface)?;
            let anim = dcomp.CreateAnimation()?;
            // 1 Hz entre 20% e 100% de opacidade. Sem `End`: repete para sempre no compositor.
            anim.AddSinusoidal(0.0, 0.6, 0.4, 1.0, 0.0)?;
            pulse.SetOpacity(&anim)?;

            let removed = register_removed_event(&d3d);
            dcomp.Commit()?;
            Ok(Self {
                d3d,
                removed,
                dcomp,
                _target: target,
                root,
                surface,
                pulse,
                pulse_on: false,
                white,
                black,
                paint,
                highlight,
                rim,
                res: dc,
                art: None,
                badge: None,
                toast_icon: None,
                backdrop: None,
                tray: Vec::new(),
            })
        }
    }

    /// Liga, desliga e posiciona o ponto de pulso (pixeis da janela).
    fn set_pulse(&mut self, on: bool, (x, y): (f32, f32)) -> Result<()> {
        // SAFETY: visuais desta cadeia, usados na thread que os criou.
        unsafe {
            if on {
                self.pulse.SetOffsetX2(x.round())?;
                self.pulse.SetOffsetY2(y.round())?;
            }
            if on != self.pulse_on {
                if on {
                    self.root.AddVisual(&self.pulse, true, None::<&IDCompositionVisual>)?;
                } else {
                    self.root.RemoveVisual(&self.pulse)?;
                }
                self.pulse_on = on;
            }
        }
        Ok(())
    }
}

/// **WARP primeiro** (o rasterizador do Direct3D no CPU). Medido num portátil com Intel UHD
/// e RTX 3050, depois de 30 s parado:
///
/// | dispositivo        | working set | privada | threads | repouso      |
/// |--------------------|-------------|---------|---------|--------------|
/// | WARP               | ~20-25 MB   | ~7-10 MB| 11-18   | 0 ciclos     |
/// | GPU integrada      | ~60 MB      | ~54 MB  | 7       | ~0,3 M/s     |
/// | GPU por omissão    | ~41 MB      | ~50 MB  | 30      | ~2,8 M/s     |
///
/// A GPU por omissão era a dedicada: ficava acordada (bateria) e uma thread do driver
/// acordava sozinha. Para um notch de ~0,2 Mpx o WARP chega (~1 ms por frame a animar) e
/// o DWM compõe o resultado na GPU na mesma. A GPU de menor consumo fica como alternativa.
fn create_d3d() -> Result<ID3D11Device> {
    let low_power = low_power_adapter();
    let attempts = [
        (None, D3D_DRIVER_TYPE_WARP),
        (low_power.as_ref(), D3D_DRIVER_TYPE_UNKNOWN),
        (None, D3D_DRIVER_TYPE_HARDWARE),
    ];
    let mut last = Error::from_hresult(E_FAIL);
    for (adapter, driver) in attempts {
        if driver == D3D_DRIVER_TYPE_UNKNOWN && adapter.is_none() {
            continue;
        }
        match create_d3d_with(adapter, driver) {
            Ok(d) => return Ok(d),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn low_power_adapter() -> Option<IDXGIAdapter> {
    // SAFETY: fábrica DXGI sem argumentos de ponteiro; o adaptador 0 é o de menor consumo.
    unsafe {
        let factory: IDXGIFactory6 = CreateDXGIFactory1().ok()?;
        factory.EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_MINIMUM_POWER).ok()
    }
}

fn create_d3d_with(adapter: Option<&IDXGIAdapter>, driver: D3D_DRIVER_TYPE) -> Result<ID3D11Device> {
    let mut device = None;
    // SAFETY: `device` é local e recebe a interface criada; com adaptador explícito o
    // tipo de driver tem de ser `UNKNOWN`, como a API exige.
    unsafe {
        D3D11CreateDevice(
            adapter,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
    }
    device.ok_or_else(|| Error::from_hresult(E_FAIL))
}

/// Evento que o Direct3D sinaliza quando o dispositivo se perde (Windows 10 1607+).
fn register_removed_event(d3d: &ID3D11Device) -> Option<(HANDLE, u32)> {
    let d4: ID3D11Device4 = d3d.cast().ok()?;
    // SAFETY: evento novo, auto-reset; se o registo falhar fecha-se logo.
    unsafe {
        let event = CreateEventW(None, false, false, None).ok()?;
        match d4.RegisterDeviceRemovedEvent(event) {
            Ok(cookie) => Some((event, cookie)),
            Err(_) => {
                let _ = CloseHandle(event);
                None
            }
        }
    }
}

impl TextKit {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: fábrica partilhada do DirectWrite, sem argumentos de ponteiro.
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let ui = first_installed(&dwrite, &["Segoe UI Variable Text", "Segoe UI"]);
        let icons = first_installed(&dwrite, &["Segoe Fluent Icons", "Segoe MDL2 Assets"]);
        let semi = DWRITE_FONT_WEIGHT_SEMI_BOLD;
        let normal = DWRITE_FONT_WEIGHT_NORMAL;
        let left = |weight, size| aligned(format(&dwrite, &ui, weight, size)?, false);
        let center = |family: &HSTRING, weight, size| aligned(format(&dwrite, family, weight, size)?, true);
        Ok(Self {
            icon: center(&icons, normal, 16.0)?,
            icon_large: center(&icons, normal, 28.0)?,
            compact: format(&dwrite, &ui, semi, 13.0)?,
            strong: left(semi, 13.0)?,
            text: left(normal, 12.0)?,
            label: left(semi, 12.0)?,
            icon_small: center(&icons, normal, 12.0)?,
            icon_tiny: center(&icons, normal, 9.0)?,
            caption: left(semi, 11.0)?,
            caption_plain: left(normal, 11.0)?,
            caption_center: center(&ui, normal, 11.0)?,
            tab: center(&ui, semi, 12.0)?,
            dwrite,
        })
    }

    pub(super) fn layout(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        max_w: f32,
        max_h: f32,
    ) -> Result<IDWriteTextLayout> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: `wide` vive durante a chamada.
        unsafe { self.dwrite.CreateTextLayout(&wide, format, max_w, max_h) }
    }

    /// Layouts para uma view. Corre só quando a view muda.
    fn content(&self, view: Option<&Presented>) -> Result<Content> {
        let Some(p) = view else { return Ok(Content::default()) };
        let c = &p.view.compact;
        let compact = p.view.priority >= Priority::Info && (!c.text.is_empty() || c.progress.is_some() || c.pulse);
        let pulse_w = if c.pulse { GAP + DOT } else { 0.0 };
        let max_text = geometry::EVENT_MAX_W - 2.0 * PAD - ICON_BOX - GAP - pulse_w;

        let toast = match &c.toast {
            Some(t) if compact => {
                let title = self.layout(&c.text, &self.strong, TOAST_TEXT_W, 20.0)?;
                let m = metrics(&title)?;
                let title = (title, m.width.min(TOAST_TEXT_W), m.height);
                let app = if t.app.is_empty() {
                    None
                } else {
                    let l = self.layout(&t.app, &self.caption_plain, TOAST_TEXT_W, 16.0)?;
                    let w = metrics(&l)?.width;
                    Some((l, w))
                };
                let body = if t.body.trim().is_empty() {
                    None
                } else {
                    let l = self.layout(t.body.trim(), &self.text, TOAST_TEXT_W, 18.0)?;
                    let w = metrics(&l)?.width.min(TOAST_TEXT_W);
                    Some((l, w))
                };
                Some(ToastContent { title, app, body })
            }
            _ => None,
        };
        let icon = self.layout(&glyph(&c.icon).to_string(), &self.icon, ICON_BOX, ICON_BOX)?;
        let text = if c.text.is_empty() {
            None
        } else {
            let layout = self.layout(&c.text, &self.compact, max_text, geometry::EVENT_H)?;
            let m = metrics(&layout)?;
            Some((layout, m.width.min(max_text), m.height))
        };
        let body_w = match (&text, c.progress) {
            (Some((_, w, _)), _) => *w,
            (None, Some(_)) => BAR_W,
            (None, None) => 0.0,
        };

        Ok(Content {
            view: Some(p.clone()),
            compact,
            icon: Some(icon),
            text,
            progress: c.progress,
            pulse: c.pulse,
            toast,
            compact_w: ICON_BOX + GAP + body_w + pulse_w,
        })
    }
}

fn metrics(layout: &IDWriteTextLayout) -> Result<DWRITE_TEXT_METRICS> {
    let mut m = DWRITE_TEXT_METRICS::default();
    // SAFETY: `m` é local.
    unsafe { layout.GetMetrics(&mut m)? };
    Ok(m)
}

/// Formato de uma linha só, cortada com reticências.
fn format(dwrite: &IDWriteFactory, family: &HSTRING, weight: DWRITE_FONT_WEIGHT, size: f32) -> Result<IDWriteTextFormat> {
    // SAFETY: strings e estruturas locais, válidas durante as chamadas.
    unsafe {
        let f = dwrite.CreateTextFormat(
            family,
            None::<&IDWriteFontCollection>,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            &HSTRING::from("pt-PT"),
        )?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        let sign = dwrite.CreateEllipsisTrimmingSign(&f)?;
        let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        f.SetTrimming(&trimming, &sign)?;
        Ok(f)
    }
}

/// Centra na vertical (para desenhar no canto de um retângulo) e, se `center`, também na horizontal.
fn aligned(f: IDWriteTextFormat, center: bool) -> Result<IDWriteTextFormat> {
    // SAFETY: formato acabado de criar, ainda sem layouts.
    unsafe {
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        if center {
            f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        }
    }
    Ok(f)
}

/// A primeira família instalada (Windows 11 tem as "Variable" e a "Fluent"; o 10 não).
fn first_installed(dwrite: &IDWriteFactory, families: &[&str]) -> HSTRING {
    let mut collection = None;
    // SAFETY: `collection`, `index` e `exists` são locais.
    unsafe {
        if dwrite.GetSystemFontCollection(&mut collection, false).is_ok()
            && let Some(collection) = collection
        {
            for name in families {
                let (mut index, mut exists) = (0, BOOL(0));
                let name = HSTRING::from(*name);
                if collection.FindFamilyName(&name, &mut index, &mut exists).is_ok() && exists.as_bool() {
                    return name;
                }
            }
        }
    }
    HSTRING::from(*families.last().expect("lista não vazia"))
}

/// Glifo da Segoe Fluent Icons / MDL2 Assets (os codepoints são os mesmos nas duas).
fn glyph(icon: &Icon) -> char {
    match icon {
        Icon::Music => '\u{E8D6}',
        Icon::Volume => '\u{E767}',
        Icon::Muted => '\u{E74F}',
        Icon::Claude => '\u{E943}',
        Icon::Mail => '\u{E715}',
        Icon::Bell => '\u{EA8F}',
        Icon::Keyboard => '\u{E765}',
        Icon::Battery => '\u{E83F}',
        Icon::BatteryCharging => '\u{E83E}',
        Icon::Headphones => '\u{E7F6}',
        Icon::Copy => '\u{E8C8}',
        Icon::Named(_) => '\u{E946}',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    #[test]
    fn artwork_is_decoded_already_reduced_to_cover_the_art_box() {
        // SAFETY: COM nesta thread de teste, antes do WIC.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
        let png = include_bytes!("../../../../docs/ui/ref-musica-detalhe.png");
        let (_, (w, h)) = decode_art(png, 90).unwrap();
        assert_eq!(w.min(h), 90, "o lado menor fica com o tamanho da capa");
        assert!(decode_art(b"isto nao e uma imagem", 90).is_err());
    }
}
