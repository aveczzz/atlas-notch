//! Arrastar e largar ficheiros (Tray), com OLE.
//!
//! - Para dentro: um `IDropTarget` registado na janela. O OLE chama-o na thread UI, dentro
//!   do nosso `DispatchMessage`, por isso os callbacks só põem `Input` na fila da janela;
//!   o loop trata do resto (abrir no separador Tray, guardar os ficheiros).
//! - Para fora: `DoDragDrop` com o `IDataObject` que o próprio Shell dá para o ficheiro
//!   (`BindToHandler(BHID_DataObject)`) e um `IDropSource` mínimo. É modal: o Windows
//!   bombeia mensagens até largar, como em qualquer app.
//! - Os ícones dos ficheiros largados são pedidos ao Shell numa thread à parte (`describe`):
//!   pode demorar dezenas de ms por ficheiro e a thread UI não espera por isso.

use super::appicon;
use super::window::{self, Input};
use island_core::event::TrayFile;
use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINTL, S_OK,
};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IBindCtx, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE, DoDragDrop, IDropSource,
    IDropSource_Impl, IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{BHID_DataObject, DragQueryFileW, HDROP, IShellItem, SHCreateItemFromParsingName};
use windows::core::{BOOL, HRESULT, HSTRING, Ref, Result, implement};

/// Regista a janela como destino de ficheiros. Precisa de `OleInitialize` nesta thread.
pub fn register(hwnd: HWND) -> Result<()> {
    let target: IDropTarget = DropTarget.into();
    // SAFETY: `hwnd` é a nossa janela, na thread dona; o OLE guarda uma referência ao target.
    unsafe { RegisterDragDrop(hwnd, &target) }
}

pub fn revoke(hwnd: HWND) {
    // SAFETY: a mesma janela registada em `register`.
    let _ = unsafe { RevokeDragDrop(hwnd) };
}

/// Nome e ícone de cada caminho. Corre fora da thread UI (pede ícones ao Shell).
pub fn describe(paths: &[String]) -> Vec<TrayFile> {
    paths
        .iter()
        .map(|path| TrayFile {
            path: path.clone(),
            name: path.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(path).to_owned(),
            icon: appicon::for_file(path),
        })
        .collect()
}

/// Arrasta o ficheiro para fora da island. Devolve o efeito final (cópia, movido, nada).
pub fn drag_out(path: &str) -> DROPEFFECT {
    let source: IDropSource = DropSource.into();
    // SAFETY: COM/OLE inicializado na thread UI; objetos locais vivos durante o `DoDragDrop`
    // (que é síncrono); `effect` é local.
    unsafe {
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(path), None::<&IBindCtx>) else {
            return DROPEFFECT_NONE;
        };
        let Ok(data) = item.BindToHandler::<_, IDataObject>(None::<&IBindCtx>, &BHID_DataObject) else {
            return DROPEFFECT_NONE;
        };
        let mut effect = DROPEFFECT_NONE;
        let hr = DoDragDrop(&data, &source, DROPEFFECT_COPY | DROPEFFECT_MOVE, &mut effect);
        if hr == DRAGDROP_S_DROP { effect } else { DROPEFFECT_NONE }
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// Os caminhos de um `IDataObject` com ficheiros (`CF_HDROP`), se tiver.
fn paths(data: &IDataObject) -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: o meio é libertado com `ReleaseStgMedium`; os buffers são locais e o
    // `DragQueryFileW` escreve no máximo o tamanho que lhe damos.
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else { return out };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        for i in 0..count.min(64) {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let written = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
            out.push(String::from_utf16_lossy(&buf[..written]));
        }
        ReleaseStgMedium(&mut medium);
    }
    out
}

fn has_files(data: &IDataObject) -> bool {
    // SAFETY: só consulta o formato.
    unsafe { data.QueryGetData(&hdrop_format()) == S_OK }
}

#[implement(IDropTarget)]
struct DropTarget;

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        _: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let files = data.as_ref().is_some_and(has_files);
        // SAFETY: o OLE dá um `effect` válido para escrevermos a resposta.
        unsafe { *effect = if files { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        if files {
            window::push(Input::DragEnter);
        }
        Ok(())
    }

    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        // SAFETY: como em `DragEnter`. Só aceitamos cópia: o ficheiro fica onde está.
        unsafe {
            if *effect != DROPEFFECT_NONE {
                *effect = DROPEFFECT_COPY;
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        window::push(Input::DragLeave);
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let files = data.as_ref().map(paths).unwrap_or_default();
        // SAFETY: como em `DragEnter`.
        unsafe { *effect = if files.is_empty() { DROPEFFECT_NONE } else { DROPEFFECT_COPY } };
        window::push(Input::Dropped(files));
        Ok(())
    }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}
