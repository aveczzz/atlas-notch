//! Ícone na bandeja do Windows (a zona das setinhas ao lado do wifi), como o TranslucentTB.
//!
//! Sem ele a island só se via no notch: não havia maneira de a esconder, abrir a configuração
//! ou fechá-la sem o Gestor de Tarefas. Clique esquerdo mostra ou esconde; clique direito abre
//! o menu (o mesmo do clique direito na island).
//!
//! O Windows 11 põe os ícones novos no menu "^": arrasta-se de lá para a barra para ficar sempre
//! à vista (uma escolha do utilizador, a app não a consegue forçar).
//!
//! O ícone é desenhado aqui (um notch num quadrado), sem ficheiros de imagem.

use super::window::{Input, push};
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, HICON, ICONINFO, RegisterWindowMessageW, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP,
};
use windows::core::w;

/// Mensagem com que o Windows avisa a janela de cliques no ícone.
pub const WM_APP_TRAY: u32 = WM_APP + 3;
/// Id do nosso ícone (só há um).
const ICON_ID: u32 = 1;
const SIZE: i32 = 32;

thread_local! {
    static ICON: Cell<HICON> = Cell::new(HICON::default());
}

/// Mensagem registada que o Explorador envia às janelas quando recria a barra de tarefas
/// (ex.: depois de ele reiniciar): os ícones da bandeja perdem-se e têm de se pôr outra vez.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// Regista a mensagem `TaskbarCreated` e põe o ícone. Falhar só deixa a island sem ícone.
pub fn register(hwnd: HWND) {
    // SAFETY: nome literal.
    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }, Ordering::Relaxed);
    if !add(hwnd) {
        log!("ícone da bandeja indisponível");
    }
}

/// A mensagem `TaskbarCreated` (0 se ainda não foi registada).
pub fn taskbar_created() -> u32 {
    TASKBAR_CREATED.load(Ordering::Relaxed)
}

fn data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    for (slot, c) in nid.szTip.iter_mut().zip("Dynamic Island".encode_utf16()) {
        *slot = c;
    }
    nid
}

/// Põe (ou repõe) o ícone na bandeja.
pub fn add(hwnd: HWND) -> bool {
    let mut icon = ICON.get();
    if icon.is_invalid() {
        icon = make_icon().unwrap_or_default();
        ICON.set(icon);
    }
    let mut nid = data(hwnd, icon);
    // SAFETY: `nid` é local e está preenchido; a janela é a nossa.
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            return false;
        }
        // Versão 4: o `lparam` traz o evento (clique, menu) nas 16 palavras baixas.
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
    true
}

/// Tira o ícone (no `WM_DESTROY`, senão fica um fantasma na bandeja até o rato passar por lá).
pub fn remove(hwnd: HWND) {
    let icon = ICON.replace(HICON::default());
    let nid = data(hwnd, icon);
    // SAFETY: o mesmo ícone registado em `add`; o `HICON` é nosso e destrói-se uma vez.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        if !icon.is_invalid() {
            let _ = DestroyIcon(icon);
        }
    }
}

/// `WM_APP_TRAY`: esquerdo mostra/esconde; direito abre o menu (tratado pela janela).
pub fn on_message(_wparam: WPARAM, lparam: LPARAM) -> Option<Click> {
    match (lparam.0 & 0xFFFF) as u32 {
        WM_LBUTTONUP => {
            push(Input::ToggleHidden);
            None
        }
        WM_CONTEXTMENU => Some(Click::Menu),
        _ => None,
    }
}

/// O que a janela tem de fazer a seguir.
#[derive(Debug, PartialEq, Eq)]
pub enum Click {
    Menu,
}

/// Distância assinada a uma caixa de cantos redondos (negativa por dentro).
fn sd_round_box(x: f32, y: f32, half_w: f32, half_h: f32, r: f32) -> f32 {
    let (qx, qy) = (x.abs() - (half_w - r), y.abs() - (half_h - r));
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Pixeis BGRA (alfa pré-multiplicado) do ícone: o notch (uma caixa escura de cantos de baixo
/// redondos, colada ao topo) com um rebordo claro, que se vê nas barras escura e clara.
fn icon_pixels() -> Vec<u8> {
    let mut px = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            // Caixa centrada em (16, 3) com 22 de largura e 22 de altura: o topo fica fora do ícone.
            let d = sd_round_box(x as f32 + 0.5 - 16.0, y as f32 + 0.5 - 3.0, 11.0, 11.0, 6.0);
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            let rim = (d + 1.6).clamp(0.0, 1.0);
            let (fill, edge) = (0.17f32, 0.96f32);
            let v = fill + (edge - fill) * rim;
            let c = (v * coverage * 255.0).round() as u8;
            px.extend_from_slice(&[c, c, c, (coverage * 255.0).round() as u8]);
        }
    }
    px
}

fn make_icon() -> Option<HICON> {
    let pixels = icon_pixels();
    // SAFETY: `pixels` vive durante a chamada (o GDI copia); as duas bitmaps são nossas e
    // apagam-se depois de o ícone as copiar.
    unsafe {
        let color = CreateBitmap(SIZE, SIZE, 1, 32, Some(pixels.as_ptr().cast()));
        let mask = CreateBitmap(SIZE, SIZE, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_notch_glued_to_the_top() {
        let px = icon_pixels();
        let alpha = |x: i32, y: i32| px[((y * SIZE + x) * 4 + 3) as usize];
        assert_eq!(px.len(), (SIZE * SIZE * 4) as usize);
        assert_eq!(alpha(16, 0), 255, "o notch toca no topo");
        assert_eq!(alpha(16, 10), 255);
        assert_eq!(alpha(1, 1), 0, "fora da caixa é transparente");
        assert_eq!(alpha(16, 30), 0, "o fundo do ícone está vazio");
        assert!(alpha(6, 13) < 255, "o canto de baixo é redondo");
    }
}
