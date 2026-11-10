//! Painel expandido: separadores Nook/Som/Tray, engrenagem e os blocos dos módulos.
//!
//! A disposição vem de `geometry::panel_layout`; os mesmos retângulos servem o desenho
//! e os cliques (`hit_test`), para nunca divergirem. Os textos ficam em cache e só se
//! recriam quando os blocos ou o separador mudam.

use super::geometry::{self, Column, PanelLayout, Rect};
use super::render::TextKit;
use island_core::{ControlId, ModuleId, Row, SessionState, Widget};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ROUNDED_RECT, ID2D1BitmapBrush1, ID2D1DeviceContext, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{IDWriteTextFormat, IDWriteTextLayout};
use windows::core::Result;
use windows_numerics::{Matrix3x2, Vector2};

const WHITE: D2D1_COLOR_F = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
const BLACK: D2D1_COLOR_F = D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
const BLUE: D2D1_COLOR_F = D2D1_COLOR_F { r: 0.039, g: 0.518, b: 1.0, a: 1.0 };
const ORANGE: D2D1_COLOR_F = D2D1_COLOR_F { r: 1.0, g: 0.624, b: 0.039, a: 1.0 };
const GREEN: D2D1_COLOR_F = D2D1_COLOR_F { r: 0.188, g: 0.82, b: 0.345, a: 1.0 };

const GLYPH_PREV: char = '\u{E892}';
const GLYPH_NEXT: char = '\u{E893}';
const GLYPH_PLAY: char = '\u{E768}';
const GLYPH_PAUSE: char = '\u{E769}';
const GLYPH_GEAR: char = '\u{E713}';
const GLYPH_MUSIC: char = '\u{E8D6}';
/// "Abrir noutra janela": o botão de abrir a sessão no VS Code.
const GLYPH_OPEN: char = '\u{E8A7}';
const GLYPH_VOLUME: char = '\u{E767}';
const GLYPH_MUTED: char = '\u{E74F}';
const GLYPH_CLOSE: char = '\u{E711}';

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Nook,
    Som,
    Tray,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Nook, Tab::Som, Tab::Tray];
    const LABELS: [&str; 3] = ["Nook", "Som", "Tray"];
}

/// O que está debaixo de um clique.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    /// Um controlo de um módulo: vira `Event::Action`.
    Control(ModuleId, ControlId),
    /// Um slider: o valor vem da posição do rato na barra (`geometry::slider_value`),
    /// ao carregar e enquanto se arrasta.
    Slider(ModuleId, ControlId, Rect),
    /// Um ficheiro da Tray: clicar abre (`open`); arrastar leva-o para fora e, se for
    /// movido, sai da lista (`remove`).
    File { module: ModuleId, open: ControlId, remove: ControlId, path: String },
    Tab(Tab),
    Gear,
}

/// Imagens já na GPU para o painel (pincel de bitmap e tamanho em pixeis).
pub type Image<'a> = Option<(&'a ID2D1BitmapBrush1, (u32, u32))>;

#[derive(Debug)]
struct MediaContent {
    module: ModuleId,
    controls: [ControlId; 3],
    art_glyph: IDWriteTextLayout,
    title: IDWriteTextLayout,
    artist: IDWriteTextLayout,
    buttons: [IDWriteTextLayout; 3],
}

#[derive(Debug)]
struct SessionContent {
    state: SessionState,
    label: IDWriteTextLayout,
    status: IDWriteTextLayout,
    open: IDWriteTextLayout,
    focus: ControlId,
    model: Option<IDWriteTextLayout>,
    /// Fração do contexto e a percentagem escrita.
    context: Option<(f32, IDWriteTextLayout)>,
}

/// Uma barra de limite da conta já com os textos prontos.
#[derive(Debug)]
struct UsageContent {
    name: IDWriteTextLayout,
    fraction: f32,
    label: IDWriteTextLayout,
}

#[derive(Debug)]
struct ClaudeContent {
    module: ModuleId,
    title: IDWriteTextLayout,
    /// "Sem sessões", quando não há nenhuma.
    empty: Option<IDWriteTextLayout>,
    sessions: Vec<SessionContent>,
    usage: Vec<UsageContent>,
}

#[derive(Debug)]
enum RowContent {
    /// Layout, altura, opacidade.
    Text(IDWriteTextLayout, f32, f32),
    Progress(f32),
}

/// Textos e retângulos do painel para um conjunto de blocos e um separador.
#[derive(Debug, Default)]
pub struct PanelContent {
    key: Option<(Vec<(ModuleId, Widget)>, Tab)>,
    tab: Tab,
    layout: Option<PanelLayout>,
    tabs: Vec<IDWriteTextLayout>,
    gear: Option<IDWriteTextLayout>,
    placeholder: Option<IDWriteTextLayout>,
    media: Option<MediaContent>,
    claude: Option<ClaudeContent>,
    rows: Vec<RowContent>,
    mixer: Option<MixerContent>,
    tray: Option<TrayContent>,
}

impl PanelContent {
    /// Recria os textos se os blocos ou o separador mudaram. Barato quando nada mudou.
    pub fn update(&mut self, t: &TextKit, widgets: &[(ModuleId, Widget)], tab: Tab) -> Result<()> {
        if self.key.as_ref().is_some_and(|(w, k)| w.as_slice() == widgets && *k == tab) {
            return Ok(());
        }
        *self = Self::build(t, widgets, tab)?;
        Ok(())
    }

    fn build(t: &TextKit, widgets: &[(ModuleId, Widget)], tab: Tab) -> Result<Self> {
        let media = widgets.iter().find_map(|(m, w)| match w {
            Widget::Media(x) => Some((*m, x)),
            _ => None,
        });
        let claude = widgets.iter().find_map(|(m, w)| match w {
            Widget::Claude(x) => Some((*m, x)),
            _ => None,
        });
        let rows: Vec<&Row> = widgets
            .iter()
            .flat_map(|(_, w)| match w {
                Widget::Rows(r) => r.as_slice(),
                _ => &[],
            })
            .collect();
        let mut columns = Vec::new();
        if media.is_some() {
            columns.push(Column::Media);
        }
        if claude.is_some() {
            columns.push(Column::Claude);
        }
        if !rows.is_empty() {
            columns.push(Column::Rows);
        }
        let l = geometry::panel_layout_with(
            &columns,
            claude.map_or(0, |(_, c)| c.sessions.len()),
            claude.map_or(0, |(_, c)| c.usage.len()),
        );
        let fit = |text: &str, format: &IDWriteTextFormat, r: &Rect| t.layout(text, format, r.w, r.h);

        let tabs = Tab::LABELS.iter().zip(&l.tabs).map(|(label, r)| fit(label, &t.tab, r)).collect::<Result<_>>()?;
        let gear = Some(fit(&GLYPH_GEAR.to_string(), &t.icon, &l.gear)?);
        let placeholder = Some(fit("Arrasta ficheiros para aqui", &t.caption_center, &l.content)?);

        let media = match (media, &l.media) {
            (Some((module, w)), Some(ml)) => {
                let play = if w.playing { GLYPH_PAUSE } else { GLYPH_PLAY };
                let button = |g: char, r: &Rect| fit(&g.to_string(), &t.icon, r);
                Some(MediaContent {
                    module,
                    controls: [w.prev, w.play_pause, w.next],
                    art_glyph: fit(&GLYPH_MUSIC.to_string(), &t.icon_large, &ml.art)?,
                    title: fit(&w.title, &t.strong, &ml.title)?,
                    artist: fit(&w.artist, &t.text, &ml.artist)?,
                    buttons: [
                        button(GLYPH_PREV, &ml.buttons[0])?,
                        button(play, &ml.buttons[1])?,
                        button(GLYPH_NEXT, &ml.buttons[2])?,
                    ],
                })
            }
            _ => None,
        };

        let claude = match (claude, l.claude_title) {
            (Some((module, w)), Some(title_rect)) => {
                let mut sessions = Vec::with_capacity(l.sessions.len());
                for (s, row) in w.sessions.iter().zip(&l.sessions) {
                    sessions.push(SessionContent {
                        state: s.state,
                        label: fit(&s.label, &t.label, &row.label)?,
                        status: fit(&s.status, &t.caption_plain, &row.status)?,
                        open: fit(&GLYPH_OPEN.to_string(), &t.icon_small, &row.button)?,
                        focus: s.focus,
                        model: match (&s.model, row.model.w > 0.0) {
                            (Some(m), true) => Some(fit(m, &t.caption_right, &row.model)?),
                            _ => None,
                        },
                        context: match (&s.context, row.ctx_bar.w > 0.0) {
                            (Some(c), true) => Some((c.fraction, fit(&c.label, &t.caption_right, &row.ctx_label)?)),
                            _ => None,
                        },
                    });
                }
                let mut usage = Vec::with_capacity(l.usage.len());
                for (u, slot) in w.usage.iter().zip(&l.usage) {
                    usage.push(UsageContent {
                        name: fit(&u.name, &t.caption_plain, &slot.name)?,
                        fraction: u.fraction,
                        label: fit(&u.label, &t.caption_right, &slot.label)?,
                    });
                }
                let empty = if w.sessions.is_empty() {
                    let r = Rect { y: title_rect.bottom() + 4.0, h: 26.0, ..title_rect };
                    Some(fit("Sem sessões", &t.text, &r)?)
                } else {
                    None
                };
                Some(ClaudeContent { module, title: fit("CLAUDE CODE", &t.caption, &title_rect)?, empty, sessions, usage })
            }
            _ => None,
        };

        let rows_w = l.columns.iter().find(|(c, _)| *c == Column::Rows).map_or(0.0, |(_, r)| r.w);
        let mut row_items = Vec::new();
        for row in rows {
            match row {
                Row::Text { primary, secondary } => {
                    row_items.push(RowContent::Text(t.layout(primary, &t.text, rows_w, 18.0)?, 18.0, 1.0));
                    if let Some(s) = secondary {
                        row_items.push(RowContent::Text(t.layout(s, &t.caption_plain, rows_w, 16.0)?, 16.0, 0.55));
                    }
                }
                Row::Progress(p) => row_items.push(RowContent::Progress(*p)),
                // Sliders e botões genéricos: fases 6 (misturador) e seguintes.
                Row::Slider { .. } | Row::Buttons(_) => {}
            }
        }

        let mixer = MixerContent::build(t, widgets, &l.content)?;
        let tray = TrayContent::build(t, widgets, &l.content)?;
        Ok(Self {
            key: Some((widgets.to_vec(), tab)),
            tab,
            layout: Some(l),
            tabs,
            gear,
            placeholder,
            media,
            claude,
            rows: row_items,
            mixer,
            tray,
        })
    }

    /// Controlo, separador ou engrenagem em (`x`, `y`), em DIPs na origem do contorno.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<Hit> {
        let l = self.layout.as_ref()?;
        if let Some(i) = l.tabs.iter().position(|r| r.contains(x, y)) {
            return Some(Hit::Tab(Tab::ALL[i]));
        }
        if l.gear.contains(x, y) {
            return Some(Hit::Gear);
        }
        match self.tab {
            Tab::Tray => return self.tray.as_ref().and_then(|t| t.hit_test(x, y)),
            Tab::Som => return self.mixer.as_ref().and_then(|m| m.hit_test(x, y)),
            Tab::Nook => {}
        }
        if let (Some(m), Some(ml)) = (&self.media, &l.media)
            && let Some(i) = ml.buttons.iter().position(|r| r.contains(x, y))
        {
            return Some(Hit::Control(m.module, m.controls[i]));
        }
        if let Some(c) = &self.claude {
            for (row, s) in l.sessions.iter().zip(&c.sessions) {
                if row.button.contains(x, y) {
                    return Some(Hit::Control(c.module, s.focus));
                }
            }
        }
        None
    }

    /// # Safety
    /// `dc` tem de estar entre `BeginDraw` e `EndDraw`.
    /// `art`: a capa; sem ela, um ícone de música. `badge`: o ícone da app, sobre o canto
    /// da capa. `tray`: os ícones dos ficheiros, pela ordem da lista. `drop_hover`: estão a
    /// arrastar ficheiros por cima (realça a zona de largar).
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn draw(
        &self,
        dc: &ID2D1DeviceContext,
        paint: &ID2D1SolidColorBrush,
        art: Image,
        badge: Image,
        tray: &[Image],
        drop_hover: bool,
        alpha: f32,
    ) {
        let Some(l) = &self.layout else { return };
        let text = |layout: &IDWriteTextLayout, r: &Rect, color: D2D1_COLOR_F, opacity: f32| {
            // SAFETY: garantido por quem chama (contexto dentro de `BeginDraw`).
            unsafe {
                paint.SetColor(&color);
                paint.SetOpacity(alpha * opacity);
                dc.DrawTextLayout(Vector2 { X: r.x, Y: r.y }, layout, paint, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        };
        let fill = |r: &Rect, radius: f32, color: D2D1_COLOR_F, opacity: f32| {
            // SAFETY: garantido por quem chama.
            unsafe {
                paint.SetColor(&color);
                paint.SetOpacity(alpha * opacity);
                let rect = D2D_RECT_F { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() };
                dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius }, paint);
            }
        };

        // Barra fina (trilho de vidro e preenchimento): branca, laranja quando está quase cheia.
        let meter = |bar: &Rect, fraction: f32| {
            fill(bar, bar.h / 2.0, WHITE, 0.16);
            let color = if fraction >= 0.85 { ORANGE } else { WHITE };
            if fraction > 0.0 {
                fill(&Rect { w: (bar.w * fraction).max(bar.h), ..*bar }, bar.h / 2.0, color, 0.9);
            }
        };

        // Cabeçalho.
        let selected = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        // Controlo segmentado: uma pílula de vidro atrás dos três, e o ativo mais claro.
        if let (Some(first), Some(last)) = (l.tabs.first(), l.tabs.last()) {
            let pill = Rect { x: first.x - 2.0, y: first.y - 2.0, w: last.right() - first.x + 4.0, h: first.h + 4.0 };
            fill(&pill, pill.h / 2.0, WHITE, 0.10);
        }
        for (i, (layout, r)) in self.tabs.iter().zip(&l.tabs).enumerate() {
            if i == selected {
                fill(r, r.h / 2.0, WHITE, 0.20);
            }
            text(layout, r, WHITE, if i == selected { 1.0 } else { 0.55 });
        }
        if let Some(g) = &self.gear {
            text(g, &l.gear, WHITE, 0.55);
        }

        match self.tab {
            Tab::Tray => {
                if drop_hover {
                    // A zona de largar acende enquanto arrastam ficheiros por cima.
                    fill(&l.content, 14.0, BLUE, 0.18);
                }
                match &self.tray {
                    Some(t) if !t.cells.is_empty() => {
                        for (i, c) in t.cells.iter().enumerate() {
                            match tray.get(i).copied().flatten() {
                                // SAFETY: garantido por quem chama.
                                Some(image) => unsafe { draw_image(dc, image, &c.cell.icon, 8.0, alpha) },
                                None => fill(&c.cell.icon, 8.0, WHITE, 0.1),
                            }
                            text(&c.name, &c.cell.name, WHITE, 0.85);
                            fill(&c.cell.remove, 8.0, WHITE, 0.12);
                            text(&c.remove_glyph, &c.cell.remove, WHITE, 0.8);
                        }
                    }
                    _ => {
                        if let Some(p) = &self.placeholder {
                            text(p, &l.content, WHITE, if drop_hover { 0.9 } else { 0.4 });
                        }
                    }
                }
                return;
            }
            Tab::Som => {
                if let Some(m) = &self.mixer {
                    // SAFETY: garantido por quem chama.
                    unsafe { m.draw(dc, paint, alpha) };
                }
                return;
            }
            Tab::Nook => {}
        }

        for d in &l.dividers {
            fill(d, 0.0, WHITE, 0.08);
        }

        if let (Some(m), Some(ml)) = (&self.media, &l.media) {
            match art {
                // SAFETY: garantido por quem chama.
                Some(image) => unsafe { draw_image(dc, image, &ml.art, 12.0, alpha) },
                None => {
                    fill(&ml.art, 12.0, WHITE, 0.1);
                    text(&m.art_glyph, &ml.art, WHITE, 0.4);
                }
            }
            if let Some(image) = badge {
                // Contorno preto à volta do badge, para se separar da capa.
                let ring = Rect { x: ml.badge.x - 2.0, y: ml.badge.y - 2.0, w: ml.badge.w + 4.0, h: ml.badge.h + 4.0 };
                fill(&ring, 7.0, BLACK, 1.0);
                // SAFETY: garantido por quem chama.
                unsafe { draw_image(dc, image, &ml.badge, 5.0, alpha) };
            }
            text(&m.title, &ml.title, WHITE, 1.0);
            text(&m.artist, &ml.artist, WHITE, 0.55);
            for (i, (b, r)) in m.buttons.iter().zip(&ml.buttons).enumerate() {
                // Círculo translúcido; o do meio (play/pausa) é mais claro.
                fill(r, r.h / 2.0, WHITE, if i == 1 { 0.20 } else { 0.10 });
                text(b, r, WHITE, 1.0);
            }
        }

        if let Some(c) = &self.claude {
            if let Some(title_rect) = &l.claude_title {
                text(&c.title, title_rect, WHITE, 0.4);
                if let Some(e) = &c.empty {
                    text(e, &Rect { y: title_rect.bottom() + 4.0, h: 26.0, ..*title_rect }, WHITE, 0.4);
                }
            }
            for (s, row) in c.sessions.iter().zip(&l.sessions) {
                let color = match s.state {
                    SessionState::Working => BLUE,
                    SessionState::NeedsInput => ORANGE,
                    SessionState::Done => GREEN,
                };
                fill(&row.dot, row.dot.w / 2.0, color, 1.0);
                text(&s.label, &row.label, WHITE, 1.0);
                text(&s.status, &row.status, WHITE, 0.55);
                fill(&row.button, row.button.h / 2.0, WHITE, 0.12);
                text(&s.open, &row.button, WHITE, 0.9);
                if let Some(model) = &s.model {
                    text(model, &row.model, WHITE, 0.45);
                }
                if let Some((fraction, label)) = &s.context {
                    meter(&row.ctx_bar, *fraction);
                    text(label, &row.ctx_label, WHITE, 0.55);
                }
            }
            // Limites da conta: "5 h ▬▬▬ 37% · 2h10" e "Semana ▬▬ 12% · 3d4h".
            for (u, slot) in c.usage.iter().zip(&l.usage) {
                text(&u.name, &slot.name, WHITE, 0.55);
                meter(&slot.bar, u.fraction);
                text(&u.label, &slot.label, WHITE, 0.7);
            }
        }

        if let Some((_, col)) = l.columns.iter().find(|(c, _)| *c == Column::Rows) {
            let mut y = col.y;
            for item in &self.rows {
                match item {
                    RowContent::Text(layout, h, opacity) => {
                        if y + h > col.bottom() {
                            break;
                        }
                        text(layout, &Rect { y, h: *h, ..*col }, WHITE, *opacity);
                        y += h;
                    }
                    RowContent::Progress(p) => {
                        let track = Rect { y: y + 4.0, h: 4.0, ..*col };
                        fill(&track, 2.0, WHITE, 0.2);
                        fill(&Rect { w: col.w * p.clamp(0.0, 1.0), ..track }, 2.0, WHITE, 0.9);
                        y += 12.0;
                    }
                }
            }
        }
    }
}

/// Um ficheiro da Tray já com os textos prontos.
#[derive(Debug)]
struct TrayCellContent {
    cell: geometry::TrayCell,
    name: IDWriteTextLayout,
    remove_glyph: IDWriteTextLayout,
    path: String,
    open: ControlId,
    remove: ControlId,
}

/// O separador "Tray": os ficheiros guardados, numa fila.
#[derive(Debug)]
struct TrayContent {
    module: ModuleId,
    cells: Vec<TrayCellContent>,
}

impl TrayContent {
    fn build(t: &TextKit, widgets: &[(ModuleId, Widget)], content: &Rect) -> Result<Option<Self>> {
        let Some((module, w)) = widgets.iter().find_map(|(m, w)| match w {
            Widget::Tray(x) => Some((*m, x)),
            _ => None,
        }) else {
            return Ok(None);
        };
        let cells = geometry::tray_cells(content, w.items.len());
        let mut out = Vec::with_capacity(cells.len());
        for (cell, item) in cells.into_iter().zip(&w.items) {
            out.push(TrayCellContent {
                name: t.layout(&item.name, &t.caption_center, cell.name.w, cell.name.h)?,
                remove_glyph: t.layout(&GLYPH_CLOSE.to_string(), &t.icon_tiny, cell.remove.w, cell.remove.h)?,
                cell,
                path: item.path.clone(),
                open: item.open,
                remove: item.remove,
            });
        }
        Ok(Some(Self { module, cells: out }))
    }

    fn hit_test(&self, x: f32, y: f32) -> Option<Hit> {
        for c in &self.cells {
            if c.cell.remove.contains(x, y) {
                return Some(Hit::Control(self.module, c.remove));
            }
            if c.cell.cell.contains(x, y) {
                return Some(Hit::File { module: self.module, open: c.open, remove: c.remove, path: c.path.clone() });
            }
        }
        None
    }
}

/// Uma linha do misturador já com os textos prontos.
#[derive(Debug)]
struct VolumeContent {
    row: geometry::VolumeRow,
    label: IDWriteTextLayout,
    percent: IDWriteTextLayout,
    /// Glifo de som/silêncio; só nas linhas com botão de mute.
    mute_glyph: Option<IDWriteTextLayout>,
    value: f32,
    muted: bool,
    slider: ControlId,
    mute: Option<ControlId>,
}

/// O separador "Som": volume geral e uma linha por app.
#[derive(Debug)]
struct MixerContent {
    module: ModuleId,
    lines: Vec<VolumeContent>,
}

impl MixerContent {
    fn build(t: &TextKit, widgets: &[(ModuleId, Widget)], content: &Rect) -> Result<Option<Self>> {
        let Some((module, w)) = widgets.iter().find_map(|(m, w)| match w {
            Widget::Mixer(x) => Some((*m, x)),
            _ => None,
        }) else {
            return Ok(None);
        };
        let rows = geometry::volume_rows(content, w.lines.len());
        let mut lines = Vec::with_capacity(rows.len());
        for (row, line) in rows.into_iter().zip(&w.lines) {
            let shown = if line.muted { 0 } else { (line.value * 100.0).round() as u32 };
            let label = t.layout(&line.label, &t.label, row.label.w, row.label.h)?;
            let percent = t.layout(&format!("{shown}%"), &t.caption_plain, row.percent.w, row.percent.h)?;
            let mute_glyph = match line.mute {
                Some(_) => {
                    let g = if line.muted { GLYPH_MUTED } else { GLYPH_VOLUME };
                    Some(t.layout(&g.to_string(), &t.icon_small, row.mute.w, row.mute.h)?)
                }
                None => None,
            };
            lines.push(VolumeContent {
                row,
                label,
                percent,
                mute_glyph,
                value: line.value,
                muted: line.muted,
                slider: line.slider,
                mute: line.mute,
            });
        }
        Ok(Some(Self { module, lines }))
    }

    fn hit_test(&self, x: f32, y: f32) -> Option<Hit> {
        for l in &self.lines {
            if let Some(mute) = l.mute
                && l.row.mute.contains(x, y)
            {
                return Some(Hit::Control(self.module, mute));
            }
            // A zona do slider é a linha toda entre o nome e a percentagem (mais fácil de apanhar).
            let zone = Rect { y: l.row.label.y, h: l.row.label.h, ..l.row.track };
            if zone.contains(x, y) {
                return Some(Hit::Slider(self.module, l.slider, l.row.track));
            }
        }
        None
    }

    /// # Safety
    /// `dc` tem de estar entre `BeginDraw` e `EndDraw`.
    unsafe fn draw(&self, dc: &ID2D1DeviceContext, paint: &ID2D1SolidColorBrush, alpha: f32) {
        // SAFETY: garantido por quem chama (contexto dentro de `BeginDraw`).
        unsafe {
            let fill = |r: &Rect, radius: f32, opacity: f32| {
                paint.SetColor(&WHITE);
                paint.SetOpacity(alpha * opacity);
                let rect = D2D_RECT_F { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() };
                dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius }, paint);
            };
            let text = |layout: &IDWriteTextLayout, r: &Rect, opacity: f32| {
                paint.SetColor(&WHITE);
                paint.SetOpacity(alpha * opacity);
                dc.DrawTextLayout(Vector2 { X: r.x, Y: r.y }, layout, paint, D2D1_DRAW_TEXT_OPTIONS_NONE);
            };
            for l in &self.lines {
                let dim = if l.muted { 0.45 } else { 1.0 };
                text(&l.label, &l.row.label, 0.9 * dim);
                // Barra grossa (8 DIP) centrada na zona do slider, como o volume do iOS:
                // trilho de vidro e preenchimento branco, sem botão.
                let t = &l.row.track;
                let bar = Rect { y: t.y + t.h / 2.0 - 4.0, h: 8.0, ..*t };
                fill(&bar, 4.0, 0.16);
                let value = if l.muted { 0.0 } else { l.value.clamp(0.0, 1.0) };
                if value > 0.0 {
                    // Pelo menos um círculo, para o preenchimento nunca ficar mais estreito que a barra é alta.
                    fill(&Rect { w: (bar.w * value).max(bar.h), ..bar }, 4.0, 0.88 * dim);
                }
                text(&l.percent, &l.row.percent, 0.55);
                if let Some(g) = &l.mute_glyph {
                    text(g, &l.row.mute, if l.muted { 0.9 } else { 0.55 });
                }
            }
        }
    }
}

/// Uma imagem (pincel de bitmap e o seu tamanho em pixeis) a cobrir `r`, com cantos de raio
/// `radius`. O bitmap tem o lado menor = `r` em pixeis; a 96 DPI do bitmap isso dá pixeis =
/// DIPs, por isso reduzir para o tamanho de `r` e centrar (corta o excesso de imagens que
/// não são quadradas, como as miniaturas de vídeos).
///
/// # Safety
/// `dc` tem de estar entre `BeginDraw` e `EndDraw`.
pub(super) unsafe fn draw_image(
    dc: &ID2D1DeviceContext,
    (brush, (w, h)): (&ID2D1BitmapBrush1, (u32, u32)),
    r: &Rect,
    radius: f32,
    alpha: f32,
) {
    let k = r.w / (w.min(h).max(1) as f32);
    let (bw, bh) = (w as f32 * k, h as f32 * k);
    let transform = Matrix3x2 {
        M11: k,
        M22: k,
        M31: r.x - (bw - r.w) / 2.0,
        M32: r.y - (bh - r.h) / 2.0,
        ..Default::default()
    };
    // SAFETY: garantido por quem chama.
    unsafe {
        brush.SetTransform(&transform);
        brush.SetOpacity(alpha);
        let rect = D2D_RECT_F { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() };
        dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius }, brush);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_core::{ClaudeWidget, MediaWidget, MixerWidget, SessionLine, TrayItem, TrayWidget, VolumeLine};

    const PLAY: ControlId = ControlId::new("play");

    fn widgets(sessions: usize) -> Vec<(ModuleId, Widget)> {
        let media = Widget::Media(MediaWidget {
            title: "Venus".into(),
            artist: "Zara Larsson".into(),
            playing: true,
            artwork: None,
            app_icon: None,
            prev: ControlId::new("prev"),
            play_pause: PLAY,
            next: ControlId::new("next"),
        });
        let lines = (0..sessions)
            .map(|i| SessionLine {
                label: format!("proj{i}"),
                status: "a editar".into(),
                state: SessionState::Working,
                focus: ControlId::indexed("focus", i as u32 + 1),
                model: None,
                context: None,
            })
            .collect();
        vec![("claude", Widget::Claude(ClaudeWidget { sessions: lines, usage: Vec::new() })), ("media", media)]
    }

    fn center(r: &Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn clicks_land_on_the_right_controls() {
        let t = TextKit::new().unwrap();
        let mut p = PanelContent::default();
        p.update(&t, &widgets(2), Tab::Nook).unwrap();
        let l = p.layout.clone().unwrap();

        let (x, y) = center(&l.media.as_ref().unwrap().buttons[1]);
        assert_eq!(p.hit_test(x, y), Some(Hit::Control("media", PLAY)));
        let (x, y) = center(&l.sessions[1].button);
        assert_eq!(p.hit_test(x, y), Some(Hit::Control("claude", ControlId::indexed("focus", 2))));
        let (x, y) = center(&l.tabs[1]);
        assert_eq!(p.hit_test(x, y), Some(Hit::Tab(Tab::Som)));
        let (x, y) = center(&l.tabs[2]);
        assert_eq!(p.hit_test(x, y), Some(Hit::Tab(Tab::Tray)));
        let (x, y) = center(&l.gear);
        assert_eq!(p.hit_test(x, y), Some(Hit::Gear));
        let (x, y) = center(&l.media.as_ref().unwrap().art);
        assert_eq!(p.hit_test(x, y), None, "a capa não é um botão");
    }

    #[test]
    fn music_column_comes_first_whatever_the_module_order() {
        let t = TextKit::new().unwrap();
        let mut p = PanelContent::default();
        p.update(&t, &widgets(0), Tab::Nook).unwrap();
        let cols: Vec<_> = p.layout.as_ref().unwrap().columns.iter().map(|(c, _)| *c).collect();
        assert_eq!(cols, vec![Column::Media, Column::Claude]);
        assert!(p.claude.as_ref().unwrap().empty.is_some(), "sem sessões mostra o aviso");
    }

    #[test]
    fn sound_tab_hits_sliders_and_mute_buttons() {
        let t = TextKit::new().unwrap();
        let mixer = Widget::Mixer(MixerWidget {
            lines: vec![
                VolumeLine { label: "Volume geral".into(), value: 0.5, muted: false, slider: ControlId::new("master"), mute: None },
                VolumeLine {
                    label: "Spotify".into(),
                    value: 0.8,
                    muted: false,
                    slider: ControlId::indexed("session", 1),
                    mute: Some(ControlId::indexed("mute", 1)),
                },
            ],
        });
        let mut p = PanelContent::default();
        p.update(&t, &[("mixer", mixer)], Tab::Som).unwrap();
        let m = p.mixer.as_ref().unwrap();
        let row = &m.lines[1].row;
        let (x, y) = center(&row.track);
        assert_eq!(p.hit_test(x, y), Some(Hit::Slider("mixer", ControlId::indexed("session", 1), row.track)));
        let (x, y) = center(&row.mute);
        assert_eq!(p.hit_test(x, y), Some(Hit::Control("mixer", ControlId::indexed("mute", 1))));
        let (x, y) = center(&m.lines[0].row.mute);
        assert_eq!(p.hit_test(x, y), None, "o geral não tem mute");
    }

    #[test]
    fn tray_cells_open_drag_and_remove() {
        let t = TextKit::new().unwrap();
        let item = |name: &str, k: u32| TrayItem {
            path: format!(r"C:x{name}"),
            name: name.into(),
            icon: None,
            open: ControlId::indexed("open", k),
            remove: ControlId::indexed("remove", k),
        };
        let tray = Widget::Tray(TrayWidget { items: vec![item("a.pdf", 1), item("b.png", 2)] });
        let mut p = PanelContent::default();
        p.update(&t, &[("tray", tray)], Tab::Tray).unwrap();
        let cells = &p.tray.as_ref().unwrap().cells;
        let (x, y) = center(&cells[1].cell.icon);
        assert_eq!(
            p.hit_test(x, y),
            Some(Hit::File {
                module: "tray",
                open: ControlId::indexed("open", 2),
                remove: ControlId::indexed("remove", 2),
                path: r"C:xb.png".into(),
            })
        );
        let (x, y) = center(&cells[0].cell.remove);
        assert_eq!(p.hit_test(x, y), Some(Hit::Control("tray", ControlId::indexed("remove", 1))), "o ✕ ganha à célula");
    }

    #[test]
    fn tray_tab_hides_the_module_controls() {
        let t = TextKit::new().unwrap();
        let mut p = PanelContent::default();
        p.update(&t, &widgets(1), Tab::Tray).unwrap();
        let l = p.layout.clone().unwrap();
        let (x, y) = center(&l.media.as_ref().unwrap().buttons[1]);
        assert_eq!(p.hit_test(x, y), None);
        let (x, y) = center(&l.tabs[0]);
        assert_eq!(p.hit_test(x, y), Some(Hit::Tab(Tab::Nook)));
    }
}
