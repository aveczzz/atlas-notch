//! Fundo desfocado do vidro.
//!
//! O Windows não desfoca o que está atrás de uma janela que nunca é ativada (ver CLAUDE.md,
//! fase 10), por isso a island faz a sua própria fotografia: quando começa a crescer a partir
//! do repouso, copia do ecrã a zona onde a janela está (GDI, uma vez), desfoca-a (Gaussian
//! Blur do Direct2D, uma vez) e guarda o resultado. Cada frame só pinta esse bitmap: não há
//! captura nem desfoque durante a animação, e em repouso a fotografia é deitada fora.
//!
//! A fotografia é tirada em repouso, com a pílula preta no sítio: essa zona é "remendada"
//! repetindo a linha logo abaixo dela, para não aparecer um retângulo preto no vidro.


use std::sync::atomic::{AtomicBool, Ordering};
use super::window::PxRect;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur, CLSID_D2D1Saturation, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
    D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_PROPERTY_TYPE_UNKNOWN,
    ID2D1Bitmap1, ID2D1DeviceContext,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::core::{Error, Interface, Result};

/// Desfoque, em DIPs (desvio padrão do Gaussian Blur).
const BLUR_DIP: f32 = 24.0;
/// A primeira fotografia de cada arranque já foi para o log.
static FIRST_LOGGED: AtomicBool = AtomicBool::new(false);
/// Cores mais vivas atrás do vidro (o `saturate(170%)` do mockup).
const SATURATION: f32 = 1.7;

/// Fotografa a zona `window` do ecrã e devolve-a desfocada, pronta a pintar. `dpi`: o da
/// janela, para o bitmap ter o tamanho certo em DIPs. `res`: contexto de recursos (só cria
/// bitmaps e desenha fora do ecrã).
pub fn capture(res: &ID2D1DeviceContext, window: PxRect, dpi: u32, cover: (f32, f32)) -> Result<ID2D1Bitmap1> {
    let (w, h) = (window.w.max(1) as usize, window.h.max(1) as usize);
    let scale = dpi as f32 / 96.0;
    let mut pixels = grab(window)?;
    // Diagnóstico para o island.log: a primeira fotografia de cada arranque, e qualquer uma preta.
    let (mean, max) = luminance(&pixels);
    if max < 8 {
        log!("vidro: a fotografia do ecrã veio PRETA ({w}x{h} px em ({},{}) a {dpi} DPI): o Windows não deixou ler o ecrã", window.x, window.y);
    } else if !FIRST_LOGGED.swap(true, Ordering::Relaxed) {
        log!("vidro: fotografia {w}x{h} px em ({},{}) a {dpi} DPI, luminância média {mean:.0}", window.x, window.y);
    }
    patch_pill(&mut pixels, w, h, scale, cover);

    let dpi = dpi as f32;
    let size = D2D_SIZE_U { width: w as u32, height: h as u32 };
    let source_props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
        dpiX: dpi,
        dpiY: dpi,
        ..Default::default()
    };
    let target_props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: dpi,
        dpiY: dpi,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
        ..Default::default()
    };
    // SAFETY: `pixels` vive durante a criação (o Direct2D copia os dados); o contexto é o de
    // recursos desta cadeia, usado na thread que o criou, e o alvo é largado no fim.
    unsafe {
        let source = res.CreateBitmap(size, Some(pixels.as_ptr().cast()), (w * 4) as u32, &source_props)?;
        let blurred = res.CreateBitmap(size, None, 0, &target_props)?;

        let effect = res.CreateEffect(&CLSID_D2D1GaussianBlur)?;
        effect.SetInput(0, &source, true);
        effect.SetValue(
            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
            D2D1_PROPERTY_TYPE_UNKNOWN,
            &BLUR_DIP.to_ne_bytes(),
        )?;
        // Borda "dura": sem isto o desfoque escurece as margens da fotografia.
        effect.SetValue(D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32, D2D1_PROPERTY_TYPE_UNKNOWN, &1u32.to_ne_bytes())?;
        // Saturação por cima do desfoque: as cores do fundo ficam vivas através do vidro.
        let saturation = res.CreateEffect(&CLSID_D2D1Saturation)?;
        saturation.SetInput(0, &effect.cast::<windows::Win32::Graphics::Direct2D::ID2D1Image>()?, true);
        saturation.SetValue(0, D2D1_PROPERTY_TYPE_UNKNOWN, &SATURATION.to_ne_bytes())?;
        let image = saturation.cast::<windows::Win32::Graphics::Direct2D::ID2D1Image>()?;

        res.SetDpi(dpi, dpi);
        res.SetTarget(&blurred);
        res.BeginDraw();
        res.DrawImage(&image, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
        let ended = res.EndDraw(None, None);
        res.SetTarget(None);
        ended?;
        Ok(blurred)
    }
}

/// Copia o retângulo do ecrã para memória (BGRA, de cima para baixo, alfa 255).
fn grab(r: PxRect) -> Result<Vec<u8>> {
    let (w, h) = (r.w.max(1), r.h.max(1));
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h, // negativo: linhas de cima para baixo
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
    // SAFETY: o DC do ecrã e o compatível são libertos no fim em todos os caminhos; o DIB
    // tem `w * h * 4` bytes, que é o que se copia, e fica selecionado até ser apagado.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(Error::from_hresult(E_FAIL));
        }
        let mem = CreateCompatibleDC(Some(screen));
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0);
        let result = match dib {
            Ok(dib) if !bits.is_null() => {
                let old = SelectObject(mem, dib.into());
                let copied = BitBlt(mem, 0, 0, w, h, Some(screen), r.x, r.y, SRCCOPY | CAPTUREBLT);
                let out = copied.map(|()| {
                    let mut v = std::slice::from_raw_parts(bits.cast::<u8>(), (w * h * 4) as usize).to_vec();
                    // O GDI não preenche o alfa: sem isto o bitmap sairia transparente.
                    v.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
                    v
                });
                SelectObject(mem, old);
                let _ = DeleteObject(dib.into());
                out
            }
            Ok(dib) => {
                let _ = DeleteObject(dib.into());
                Err(Error::from_hresult(E_FAIL))
            }
            Err(e) => Err(e),
        };
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}

/// A forma que a island tem no momento da fotografia (a pílula em repouso, ou o notch largo de
/// um evento) está no meio de cima da fotografia: repete a linha logo abaixo dela por cima, para
/// o desfoque não ter um retângulo preto. `cover`: meia largura e altura da forma, em DIPs.
fn patch_pill(pixels: &mut [u8], w: usize, h: usize, scale: f32, cover: (f32, f32)) {
    let half = (cover.0 * scale).ceil() as usize + 2;
    let pill_h = (cover.1 * scale).ceil() as usize + 2;
    let (x0, x1) = ((w / 2).saturating_sub(half), (w / 2 + half).min(w));
    let src = (pill_h + 2).min(h.saturating_sub(1));
    if src <= pill_h || x1 <= x0 {
        return;
    }
    let row: Vec<u8> = pixels[(src * w + x0) * 4..(src * w + x1) * 4].to_vec();
    for y in 0..pill_h.min(h) {
        pixels[(y * w + x0) * 4..(y * w + x1) * 4].copy_from_slice(&row);
    }
}

/// Luminância média e máxima (0-255) de uma amostra dos pixeis BGRA: para o log dizer se a
/// fotografia veio preta (o Windows não deixou ler o ecrã) em vez de o vidro só "não desfocar".
fn luminance(pixels: &[u8]) -> (f32, u8) {
    let (mut sum, mut max, mut n) = (0u64, 0u8, 0u64);
    for p in pixels.as_chunks::<4>().0.iter().step_by(37) {
        let l = ((p[2] as u32 * 54 + p[1] as u32 * 183 + p[0] as u32 * 19) >> 8) as u8;
        sum += l as u64;
        max = max.max(l);
        n += 1;
    }
    (if n == 0 { 0.0 } else { sum as f32 / n as f32 }, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::geometry::{EAR, REST};

    #[test]
    fn the_pill_area_is_replaced_by_the_row_below_it() {
        let (w, h) = (400usize, 120usize);
        // Fundo cinzento (200) com a pílula preta (0) no meio de cima.
        let mut px = vec![200u8; w * h * 4];
        let half = ((REST.w / 2.0 + EAR) as usize).min(w / 2);
        for y in 0..REST.h as usize {
            for x in (w / 2 - half)..(w / 2 + half) {
                px[(y * w + x) * 4..(y * w + x) * 4 + 3].fill(0);
            }
        }
        patch_pill(&mut px, w, h, 1.0, (REST.w / 2.0 + EAR, REST.h));
        let at = |x: usize, y: usize| px[(y * w + x) * 4];
        assert_eq!(at(w / 2, 5), 200, "a pílula foi remendada");
        assert_eq!(at(w / 2, 20), 200);
        assert_eq!(at(10, 5), 200, "fora da pílula ficou igual");
    }
}
