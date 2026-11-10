//! Geometria do notch, em DIPs. Pura e testável: o renderer desenha o path do D2D e a
//! janela recorta a região a partir do **mesmo** contorno, para nunca divergirem.
//!
//! Origem no centro da borda de cima do ecrã: `x` cresce para a direita, `y` para baixo.
//!
//! ```text
//!  ___                         ___      <- y = 0 (borda do ecrã)
//!     \_ orelha       orelha _/
//!      |                     |
//!      \_________ ___________/          <- y = h, cantos de raio r
//! ```

/// Raio das orelhas côncavas que fundem o notch com a borda do ecrã.
pub const EAR: f32 = 12.0;
/// Em repouso: só o notch preto.
pub const REST: Size = Size { w: 190.0, h: 32.0, r: 14.0 };
/// A mostrar um evento (texto, HUD de volume): a largura acompanha o conteúdo.
pub const EVENT_H: f32 = 40.0;
pub const EVENT_R: f32 = 20.0;
pub const EVENT_MAX_W: f32 = 360.0;
/// Notificação de uma app: ícone, título e uma linha de texto.
pub const TOAST: Size = Size { w: 380.0, h: 72.0, r: 26.0 };
/// Ícone da app no toast e margem lateral.
pub const TOAST_ICON: f32 = 40.0;
pub const TOAST_PAD: f32 = 18.0;
/// Painel completo (hover).
pub const EXPANDED: Size = Size { w: 640.0, h: 176.0, r: 38.0 };
/// Margem lateral do conteúdo compacto.
pub const PAD: f32 = 16.0;
/// Largura do brilho azul à volta da forma (fica por fora: a região da janela alarga isto).
pub const GLOW: f32 = 4.0;

/// A forma alargada pelo brilho, para a região da janela não o cortar.
pub fn with_glow(s: Size) -> Size {
    Size { w: s.w + 2.0 * GLOW, h: s.h + GLOW, r: s.r + GLOW }
}

/// Pontos por arco no polígono. Com 12, o erro máximo no raio de 24 DIP é ~0,05 DIP.
pub const ARC_STEPS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
    pub r: f32,
}

impl Size {
    /// Forma que cobre as duas (para a região durante uma transição).
    pub fn envelope(self, other: Size) -> Size {
        Size { w: self.w.max(other.w), h: self.h.max(other.h), r: self.r.min(other.r) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Rest,
    /// `content_w`: largura do conteúdo compacto (ícone + texto + extras), sem margens.
    Event { content_w: f32 },
    /// Notificação de uma app (toast rico).
    Toast,
    Expanded,
}

pub fn target(mode: Mode) -> Size {
    match mode {
        Mode::Rest => REST,
        Mode::Event { content_w } => Size {
            w: (content_w + 2.0 * PAD).clamp(REST.w, EVENT_MAX_W),
            h: EVENT_H,
            r: EVENT_R,
        },
        Mode::Toast => TOAST,
        Mode::Expanded => EXPANDED,
    }
}

/// Contorno fechado, no sentido dos ponteiros do relógio, começando no topo da orelha esquerda.
/// Os arcos ligam-se por segmentos retos implícitos.
pub fn outline(s: Size, steps: usize) -> Vec<(f32, f32)> {
    let (hw, h) = (s.w / 2.0, s.h);
    // A mola pode passar por valores pequenos; o raio nunca excede o que cabe.
    let r = s.r.clamp(0.0, (h - EAR).min(hw).max(0.0));
    let arcs = [
        // Orelha esquerda: quarto superior direito do círculo, de cima para a direita.
        ((-hw - EAR, EAR), EAR, -90.0, 0.0),
        // Canto inferior esquerdo: da esquerda para baixo.
        ((-hw + r, h - r), r, 180.0, 90.0),
        // Canto inferior direito: de baixo para a direita.
        ((hw - r, h - r), r, 90.0, 0.0),
        // Orelha direita: quarto superior esquerdo, da esquerda para cima.
        ((hw + EAR, EAR), EAR, 180.0, 270.0),
    ];
    let mut pts = Vec::with_capacity(arcs.len() * (steps + 1));
    for ((cx, cy), radius, a0, a1) in arcs {
        for i in 0..=steps {
            let a = f32::to_radians(a0 + (a1 - a0) * i as f32 / steps as f32);
            pts.push((cx + radius * a.cos(), cy + radius * a.sin()));
        }
    }
    pts
}

/// O ponto (DIPs, origem no centro da borda de cima) está dentro da forma `s`?
/// Serve o hover: só a forma visível conta, não a região da janela (que, a encolher,
/// ainda tem o tamanho antigo).
pub fn contains(s: Size, x: f32, y: f32) -> bool {
    let pts = outline(s, ARC_STEPS / 2);
    // Par-ímpar: conta quantas arestas um raio horizontal para a direita atravessa.
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let ((xi, yi), (xj, yj)) = (pts[i], pts[j]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// O contorno em pixeis da janela, para a região (`CreatePolygonRgn`).
pub fn polygon_px(s: Size, scale: f32, center_x_px: i32) -> Vec<(i32, i32)> {
    outline(s, ARC_STEPS / 2)
        .into_iter()
        .map(|(x, y)| (center_x_px + (x * scale).round() as i32, (y * scale).round() as i32))
        .collect()
}

/// Retângulo em DIPs, na mesma origem do contorno (centro da borda de cima).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Colunas do separador Nook, pela ordem em que aparecem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Media,
    Claude,
    Rows,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaLayout {
    pub art: Rect,
    /// Ícone da app, sobre o canto inferior direito da capa.
    pub badge: Rect,
    pub title: Rect,
    pub artist: Rect,
    /// Anterior, tocar/pausa, seguinte.
    pub buttons: [Rect; 3],
}

/// Uma sessão do Claude em duas linhas (projeto por cima, estado por baixo), para caber
/// numa coluna estreita quando há três colunas.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub dot: Rect,
    pub label: Rect,
    pub status: Rect,
    /// "Abrir no VS Code" (ícone).
    pub button: Rect,
    /// Modelo, à direita do nome (largura 0 se a coluna é estreita).
    pub model: Rect,
    /// Barra de contexto e a sua percentagem, à direita do estado (largura 0 se estreita).
    pub ctx_bar: Rect,
    pub ctx_label: Rect,
}

/// Uma barra de limite da conta: nome, barra e percentagem.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSlot {
    pub name: Rect,
    pub bar: Rect,
    pub label: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PanelLayout {
    /// Nook, Som, Tray.
    pub tabs: [Rect; 3],
    pub gear: Rect,
    /// Área de conteúdo abaixo do cabeçalho (também usada pelo separador Tray).
    pub content: Rect,
    pub columns: Vec<(Column, Rect)>,
    /// Linhas verticais entre colunas.
    pub dividers: Vec<Rect>,
    pub media: Option<MediaLayout>,
    pub claude_title: Option<Rect>,
    pub sessions: Vec<SessionRow>,
    /// Barras dos limites da conta, lado a lado por baixo do título do Claude.
    pub usage: Vec<UsageSlot>,
}

pub const PANEL_PAD: f32 = 24.0;
const HEADER_Y: f32 = 10.0;
const HEADER_H: f32 = 26.0;
const TAB_W: f32 = 64.0;
const COLUMN_GAP: f32 = 28.0;
/// Larguras fixas quando há mais de uma coluna; o Claude fica com o resto.
const MEDIA_W: f32 = 260.0;
const ROWS_W: f32 = 150.0;
pub const ART: f32 = 72.0;
/// Ícone da app, no canto inferior direito da capa, a sair um pouco dela.
pub const BADGE: f32 = 22.0;
const BUTTON: f32 = 28.0;
const SESSION_H: f32 = 32.0;
const OPEN: f32 = 26.0;
/// Coluna do Claude a partir da qual há limites, modelo e barra de contexto.
const WIDE_CLAUDE: f32 = 230.0;
const USAGE_H: f32 = 14.0;

/// Disposição do painel expandido. O mesmo resultado serve para desenhar e para os cliques.
#[cfg(test)]
pub fn panel_layout(columns: &[Column], sessions: usize) -> PanelLayout {
    panel_layout_with(columns, sessions, 0)
}

/// Como `panel_layout`, com `usage` barras de limites da conta (0 a 2) por baixo do título do Claude.
pub fn panel_layout_with(columns: &[Column], sessions: usize, usage: usize) -> PanelLayout {
    let s = EXPANDED;
    let left = -s.w / 2.0 + PANEL_PAD;
    let right = s.w / 2.0 - PANEL_PAD;
    // Separadores centrados no notch (os três juntos, com 4 DIP entre eles).
    let tabs_w = 3.0 * TAB_W + 2.0 * 4.0;
    let tab = |i: f32| Rect { x: -tabs_w / 2.0 + i * (TAB_W + 4.0), y: HEADER_Y, w: TAB_W, h: HEADER_H };
    let content = Rect { x: left, y: HEADER_Y + HEADER_H + 10.0, w: right - left, h: 0.0 };
    let content = Rect { h: s.h - 16.0 - content.y, ..content };

    // Com mais de uma coluna, música e apps externas têm largura fixa e as restantes
    // (o Claude) dividem o que sobra. Sozinha, uma coluna ocupa tudo.
    let n = columns.len();
    let fixed_w = |c: Column| match c {
        _ if n == 1 => None,
        Column::Media => Some(MEDIA_W),
        Column::Rows if columns.contains(&Column::Claude) => Some(ROWS_W),
        _ => None,
    };
    let gaps = COLUMN_GAP * n.saturating_sub(1) as f32;
    let fixed: f32 = columns.iter().filter_map(|c| fixed_w(*c)).sum();
    let flexible = columns.iter().filter(|c| fixed_w(**c).is_none()).count().max(1) as f32;
    let flex_w = (content.w - gaps - fixed) / flexible;
    let mut x = content.x;
    let mut out_columns = Vec::with_capacity(n);
    let mut dividers = Vec::new();
    for (i, &c) in columns.iter().enumerate() {
        let w = fixed_w(c).unwrap_or(flex_w);
        out_columns.push((c, Rect { x, y: content.y, w, h: content.h }));
        x += w;
        if i + 1 < n {
            dividers.push(Rect { x: x + COLUMN_GAP / 2.0 - 0.5, y: content.y, w: 1.0, h: content.h });
            x += COLUMN_GAP;
        }
    }

    let media = out_columns.iter().find(|(c, _)| *c == Column::Media).map(|(_, r)| {
        let art = Rect { x: r.x, y: r.y + (r.h - ART) / 2.0, w: ART, h: ART };
        let tx = art.right() + 14.0;
        let tw = r.right() - tx;
        let button = |i: f32| Rect { x: tx - 6.0 + i * (BUTTON + 6.0), y: art.bottom() - BUTTON, w: BUTTON, h: BUTTON };
        MediaLayout {
            badge: Rect { x: art.right() - BADGE + 6.0, y: art.bottom() - BADGE + 6.0, w: BADGE, h: BADGE },
            art,
            title: Rect { x: tx, y: art.y, w: tw, h: 20.0 },
            artist: Rect { x: tx, y: art.y + 20.0, w: tw, h: 18.0 },
            buttons: [button(0.0), button(1.0), button(2.0)],
        }
    });

    let claude = out_columns.iter().find(|(c, _)| *c == Column::Claude).map(|(_, r)| *r);
    let claude_title = claude.map(|r| Rect { h: 18.0, ..r });
    // Com a coluna larga cabem os limites da conta, o modelo e a barra de contexto; com três
    // colunas (a do Claude estreita) fica só o essencial.
    let wide = claude.is_some_and(|r| r.w >= WIDE_CLAUDE);
    let usage_n = if wide { usage.min(2) } else { 0 };
    let usage_h = if usage_n > 0 { USAGE_H + 4.0 } else { 0.0 };
    let usage_slots = match claude {
        Some(r) if usage_n > 0 => {
            let seg_w = (r.w - 14.0) / 2.0;
            (0..usage_n)
                .map(|i| {
                    let x = r.x + i as f32 * (seg_w + 14.0);
                    let y = r.y + 20.0;
                    let name = Rect { x, y, w: 46.0, h: USAGE_H };
                    let label = Rect { x: x + seg_w - 62.0, y, w: 62.0, h: USAGE_H };
                    let bar = Rect { x: name.right() + 4.0, y: y + (USAGE_H - 4.0) / 2.0, w: label.x - name.right() - 8.0, h: 4.0 };
                    UsageSlot { name, bar, label }
                })
                .collect()
        }
        _ => Vec::new(),
    };
    let rows = claude.map_or(0, |r| ((r.h - 22.0 - usage_h) / SESSION_H).floor().max(0.0) as usize);
    let sessions = claude
        .map(|r| {
            (0..sessions.min(rows))
                .map(|i| {
                    let y = r.y + 22.0 + usage_h + i as f32 * SESSION_H;
                    let button = Rect { x: r.right() - OPEN, y: y + (SESSION_H - OPEN) / 2.0 - 2.0, w: OPEN, h: OPEN };
                    let none = Rect { x: button.x, y, w: 0.0, h: 0.0 };
                    let (model, ctx_bar, ctx_label) = if wide {
                        let ctx_label = Rect { x: button.x - 6.0 - 32.0, y: y + 14.0, w: 32.0, h: 14.0 };
                        let ctx_bar = Rect { x: ctx_label.x - 6.0 - 48.0, y: y + 14.0 + 5.0, w: 48.0, h: 4.0 };
                        (Rect { x: button.x - 6.0 - 70.0, y, w: 70.0, h: 16.0 }, ctx_bar, ctx_label)
                    } else {
                        (none, none, none)
                    };
                    // O texto do estado acaba onde começa a barra de contexto (ou o botão).
                    let status_end = if ctx_bar.w > 0.0 { ctx_bar.x - 8.0 } else { button.x - 6.0 };
                    let label_end = if model.w > 0.0 { model.x - 6.0 } else { button.x - 6.0 };
                    SessionRow {
                        dot: Rect { x: r.x, y: y + 5.0, w: 6.0, h: 6.0 },
                        label: Rect { x: r.x + 14.0, y, w: label_end - (r.x + 14.0), h: 16.0 },
                        status: Rect { x: r.x + 14.0, y: y + 15.0, w: status_end - (r.x + 14.0), h: 14.0 },
                        button,
                        model,
                        ctx_bar,
                        ctx_label,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    PanelLayout {
        tabs: [tab(0.0), tab(1.0), tab(2.0)],
        gear: Rect { x: right - HEADER_H, y: HEADER_Y, w: HEADER_H, h: HEADER_H },
        content,
        columns: out_columns,
        dividers,
        media,
        claude_title,
        usage: usage_slots,
        sessions,
    }
}

/// Uma linha do separador "Som".
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeRow {
    pub label: Rect,
    /// A barra inteira: o valor vem da posição do rato dentro dela.
    pub track: Rect,
    pub percent: Rect,
    pub mute: Rect,
}

const VOLUME_ROW: f32 = 23.0;
const VOLUME_LABEL: f32 = 130.0;
const PERCENT_W: f32 = 38.0;
const MUTE_W: f32 = 24.0;

/// Linhas do misturador dentro de `content`, até caberem (a 1.ª é o volume geral).
pub fn volume_rows(content: &Rect, lines: usize) -> Vec<VolumeRow> {
    let fit = (content.h / VOLUME_ROW).floor() as usize;
    (0..lines.min(fit))
        .map(|i| {
            let y = content.y + i as f32 * VOLUME_ROW;
            let mute = Rect { x: content.right() - MUTE_W, y, w: MUTE_W, h: VOLUME_ROW };
            let percent = Rect { x: mute.x - 6.0 - PERCENT_W, y, w: PERCENT_W, h: VOLUME_ROW };
            let label = Rect { x: content.x, y, w: VOLUME_LABEL, h: VOLUME_ROW };
            let track_x = label.right() + 10.0;
            VolumeRow {
                label,
                track: Rect { x: track_x, y: y + 4.0, w: percent.x - 10.0 - track_x, h: VOLUME_ROW - 8.0 },
                percent,
                mute,
            }
        })
        .collect()
}

/// Um ficheiro no separador Tray: ícone, nome por baixo e o ✕ no canto.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayCell {
    pub cell: Rect,
    pub icon: Rect,
    pub name: Rect,
    pub remove: Rect,
}

const TRAY_CELL_W: f32 = 80.0;
const TRAY_GAP: f32 = 4.0;
pub const TRAY_ICON: f32 = 48.0;

/// Células numa fila, da esquerda para a direita, até caberem.
pub fn tray_cells(content: &Rect, n: usize) -> Vec<TrayCell> {
    let fit = ((content.w + TRAY_GAP) / (TRAY_CELL_W + TRAY_GAP)).floor() as usize;
    (0..n.min(fit))
        .map(|i| {
            let x = content.x + i as f32 * (TRAY_CELL_W + TRAY_GAP);
            let cell = Rect { x, y: content.y, w: TRAY_CELL_W, h: content.h };
            let icon = Rect { x: x + (TRAY_CELL_W - TRAY_ICON) / 2.0, y: content.y + 12.0, w: TRAY_ICON, h: TRAY_ICON };
            TrayCell {
                cell,
                icon,
                name: Rect { x, y: icon.bottom() + 6.0, w: TRAY_CELL_W, h: 16.0 },
                remove: Rect { x: cell.right() - 18.0, y: content.y + 4.0, w: 16.0, h: 16.0 },
            }
        })
        .collect()
}

/// Valor do slider (0 a 1) na posição `x` dentro da barra.
pub fn slider_value(track: &Rect, x: f32) -> f32 {
    ((x - track.x) / track.w.max(1.0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_cells_fit_one_row_of_seven() {
        let l = panel_layout(&[], 0);
        let cells = tray_cells(&l.content, 20);
        assert_eq!(cells.len(), 7);
        for c in &cells {
            for part in [&c.icon, &c.name, &c.remove] {
                assert!(inside(part, &c.cell));
            }
            assert!(inside(&c.cell, &l.content));
        }
        assert!(cells[0].cell.right() < cells[1].cell.x);
    }

    #[test]
    fn hover_counts_only_inside_the_visible_shape() {
        assert!(contains(REST, 0.0, 16.0), "centro do notch");
        assert!(!contains(REST, 0.0, 60.0), "por baixo do notch");
        assert!(!contains(REST, 200.0, 10.0), "ao lado, onde estava o painel");
        assert!(contains(EXPANDED, 200.0, 100.0), "mas dentro do painel aberto");
        assert!(!contains(EXPANDED, 0.0, EXPANDED.h + 5.0));
    }

    #[test]
    fn volume_rows_fit_and_slider_maps_the_whole_track() {
        let l = panel_layout(&[], 0);
        let rows = volume_rows(&l.content, 20);
        assert!(!rows.is_empty() && rows.len() < 20, "as linhas a mais ficam de fora");
        for r in &rows {
            for part in [&r.label, &r.track, &r.percent, &r.mute] {
                assert!(inside(part, &l.content));
            }
            assert!(r.label.right() < r.track.x && r.track.right() < r.percent.x && r.percent.right() < r.mute.x);
        }
        let t = rows[0].track;
        assert_eq!(slider_value(&t, t.x - 50.0), 0.0);
        assert_eq!(slider_value(&t, t.right() + 50.0), 1.0);
        assert!((slider_value(&t, t.x + t.w / 2.0) - 0.5).abs() < 1e-6);
    }

    fn inside(inner: &Rect, outer: &Rect) -> bool {
        inner.x >= outer.x - 1e-3
            && inner.y >= outer.y - 1e-3
            && inner.right() <= outer.right() + 1e-3
            && inner.bottom() <= outer.bottom() + 1e-3
    }

    #[test]
    fn panel_fits_inside_the_expanded_shape() {
        let l = panel_layout(&[Column::Media, Column::Claude], 9);
        let body = Rect { x: -EXPANDED.w / 2.0, y: 0.0, w: EXPANDED.w, h: EXPANDED.h };
        assert!(l.tabs.iter().chain([&l.gear]).all(|r| inside(r, &body)));
        assert!(l.columns.iter().all(|(_, r)| inside(r, &l.content)));
        let m = l.media.as_ref().unwrap();
        let media_col = l.columns[0].1;
        assert!(m.buttons.iter().chain([&m.art, &m.title]).all(|r| inside(r, &media_col)));
        assert!(!l.sessions.is_empty() && l.sessions.len() < 9, "as sessões a mais ficam de fora");
        let claude_col = l.columns[1].1;
        assert!(l.sessions.iter().all(|s| {
            [&s.button, &s.label, &s.status, &s.dot].iter().all(|r| inside(r, &claude_col))
        }));
    }

    #[test]
    fn claude_keeps_room_for_its_text_with_three_columns() {
        let l = panel_layout(&[Column::Media, Column::Claude, Column::Rows], 2);
        let claude = l.columns[1].1;
        assert_eq!(l.columns[2].1.w, ROWS_W);
        assert!(l.sessions[0].label.w >= 75.0, "texto da sessão com {} DIP", l.sessions[0].label.w);
        assert!(claude.right() < l.columns[2].1.x);
    }

    #[test]
    fn columns_do_not_overlap_and_have_dividers_between() {
        let l = panel_layout(&[Column::Media, Column::Claude, Column::Rows], 1);
        assert_eq!(l.dividers.len(), 2);
        for pair in l.columns.windows(2) {
            assert!(pair[0].1.right() < pair[1].1.x);
        }
        assert_eq!(l.columns[0].1.w, MEDIA_W);
    }

    #[test]
    fn a_single_column_takes_the_whole_width() {
        let l = panel_layout(&[Column::Claude], 0);
        assert_eq!(l.columns[0].1.w, l.content.w);
        assert!(l.media.is_none() && l.sessions.is_empty() && l.dividers.is_empty());
    }

    #[test]
    fn buttons_do_not_overlap() {
        let m = panel_layout(&[Column::Media], 0).media.unwrap();
        assert!(m.buttons[0].right() <= m.buttons[1].x && m.buttons[1].right() <= m.buttons[2].x);
        assert!(m.buttons[1].contains(m.buttons[1].x + 1.0, m.buttons[1].y + 1.0));
        assert!(!m.buttons[1].contains(m.buttons[0].x + 1.0, m.buttons[0].y + 1.0));
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn outline_is_symmetric_and_touches_the_top() {
        let pts = outline(EXPANDED, ARC_STEPS);
        let n = pts.len();
        for i in 0..n {
            let (x, y) = pts[i];
            let (mx, my) = pts[n - 1 - i];
            assert!(close(x, -mx) && close(y, my), "ponto {i}: {:?} vs {:?}", pts[i], pts[n - 1 - i]);
        }
        assert!(close(pts[0].1, 0.0) && close(pts[n - 1].1, 0.0));
        let min_x = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min);
        let max_y = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        assert!(close(min_x, -(EXPANDED.w / 2.0 + EAR)));
        assert!(close(max_y, EXPANDED.h));
    }

    #[test]
    fn tiny_sizes_do_not_produce_nan() {
        let pts = outline(Size { w: 4.0, h: 3.0, r: 24.0 }, ARC_STEPS);
        assert!(pts.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
    }

    #[test]
    fn event_width_follows_content_within_limits() {
        assert_eq!(target(Mode::Event { content_w: 10.0 }).w, REST.w);
        assert_eq!(target(Mode::Event { content_w: 200.0 }).w, 200.0 + 2.0 * PAD);
        assert_eq!(target(Mode::Event { content_w: 1000.0 }).w, EVENT_MAX_W);
        assert_eq!(target(Mode::Rest), REST);
        assert_eq!(target(Mode::Toast), TOAST);
        const { assert!(TOAST.h > EVENT_H && TOAST.w > EVENT_MAX_W) };
    }

    #[test]
    fn envelope_covers_both() {
        let e = REST.envelope(EXPANDED);
        assert_eq!(e, Size { w: EXPANDED.w, h: EXPANDED.h, r: REST.r });
    }

    #[test]
    fn polygon_scales_to_pixels() {
        let p = polygon_px(REST, 1.25, 400);
        let min_x = p.iter().map(|p| p.0).min().unwrap();
        let max_x = p.iter().map(|p| p.0).max().unwrap();
        let max_y = p.iter().map(|p| p.1).max().unwrap();
        assert_eq!((min_x, max_x, max_y), (400 - 134, 400 + 134, 40)); // (95 + 12) * 1,25 = 133,75
    }

    #[test]
    fn wide_claude_column_gets_model_context_and_limits() {
        let l = panel_layout_with(&[Column::Media, Column::Claude], 2, 2);
        assert_eq!(l.usage.len(), 2);
        // As duas barras ficam lado a lado, dentro da coluna, por baixo do título.
        let (_, col) = *l.columns.iter().find(|(c, _)| *c == Column::Claude).unwrap();
        assert!(l.usage[0].label.right() <= l.usage[1].name.x && l.usage[1].label.right() <= col.right() + 0.01);
        assert!(l.usage[0].name.y >= l.claude_title.unwrap().bottom() - 0.01);
        for s in &l.sessions {
            assert!(s.model.w > 0.0 && s.ctx_bar.w > 0.0 && s.ctx_label.w > 0.0);
            assert!(s.label.right() <= s.model.x && s.status.right() <= s.ctx_bar.x, "o texto não passa para cima do resto");
            assert!(s.ctx_label.right() <= s.button.x);
            assert!(s.status.y >= l.usage[0].name.bottom(), "as sessões ficam por baixo dos limites");
        }
        assert_eq!(l.sessions.len(), 2, "cabem duas sessões com os limites");
    }

    #[test]
    fn narrow_claude_column_keeps_only_the_essentials() {
        let l = panel_layout_with(&[Column::Media, Column::Claude, Column::Rows], 1, 2);
        assert!(l.usage.is_empty());
        let s = &l.sessions[0];
        assert!(s.model.w == 0.0 && s.ctx_bar.w == 0.0);
        assert!(s.status.w > 0.0 && s.label.w > 0.0);
    }
}
