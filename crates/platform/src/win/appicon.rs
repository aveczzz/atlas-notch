//! Ícone da app que está a tocar, a partir do AUMID que o GSMTC dá.
//!
//! Um só caminho para os dois tipos de app, sem ficheiros nossos:
//! 1. `shell:AppsFolder\<AUMID>` — apps da Store (`Microsoft.ZuneMusic_...!...`) e apps de
//!    desktop que se registam com um AUMID (Chrome, Edge).
//! 2. O executável a correr com esse nome (`Spotify.exe`, ou `<AUMID>.exe`).
//!
//! O Shell dá um `HBITMAP`; codifica-se em PNG com o WIC para seguir o mesmo caminho da
//! capa (`ImageData` -> descodificação no renderer). Corre na thread `media`, uma vez por
//! app, nunca na thread UI.

use super::process;
use island_core::event::ImageData;
use windows::Win32::Foundation::{CloseHandle, SIZE};
use windows::Win32::Graphics::Gdi::{DeleteObject, HBITMAP, HPALETTE};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapEncoderNoCache, WICBitmapUsePremultipliedAlpha,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IBindCtx, STREAM_SEEK_CUR, STREAM_SEEK_SET};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SHCreateMemStream, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
};
use windows::core::HSTRING;

/// Lado do ícone em pixeis: chega para o badge (22 DIP) até 250% de escala.
const SIDE: i32 = 64;

pub fn for_app(aumid: &str) -> Option<ImageData> {
    if aumid.is_empty() {
        return None;
    }
    let bitmap = shell_image(&format!("shell:AppsFolder\\{aumid}"))
        .or_else(|| exe_path(aumid).and_then(|path| shell_image(&path)))?;
    to_png(bitmap)
}

/// Ícone de um ficheiro ou pasta (o mesmo que o Explorador mostra), para a Tray.
pub fn for_file(path: &str) -> Option<ImageData> {
    to_png(shell_image(path)?)
}

fn to_png(bitmap: HBITMAP) -> Option<ImageData> {
    let png = encode_png(bitmap);
    // SAFETY: o bitmap é nosso (devolvido pelo `GetImage`) e já foi copiado para o PNG.
    unsafe {
        let _ = DeleteObject(bitmap.into());
    }
    png
}

fn shell_image(path: &str) -> Option<HBITMAP> {
    // SAFETY: COM está inicializado na thread `media`; a string vive durante a chamada.
    unsafe {
        let item: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>).ok()?;
        item.GetImage(SIZE { cx: SIDE, cy: SIDE }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()
    }
}

/// Caminho do executável a correr com o nome `aumid` ou `aumid.exe`.
fn exe_path(aumid: &str) -> Option<String> {
    let wanted = aumid.to_lowercase();
    let wanted_exe = format!("{wanted}.exe");
    // SAFETY: snapshot e handles fechados no fim; estruturas locais com `dwSize` preenchido.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut pid = None;
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
            if name == wanted || name == wanted_exe {
                pid = Some(entry.th32ProcessID);
                break;
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        process::image_path(pid?)
    }
}

fn encode_png(bitmap: HBITMAP) -> Option<ImageData> {
    // SAFETY: COM inicializado; todos os objetos são locais e o stream é lido depois do
    // `Commit`, com o tamanho que ele próprio indica.
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let source = wic.CreateBitmapFromHBITMAP(bitmap, HPALETTE::default(), WICBitmapUsePremultipliedAlpha).ok()?;
        let (mut w, mut h) = (0, 0);
        source.GetSize(&mut w, &mut h).ok()?;
        let stream = SHCreateMemStream(None)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).ok()?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut()).ok()?;
        let frame = frame?;
        frame.Initialize(None).ok()?;
        frame.SetSize(w, h).ok()?;
        let mut format = GUID_WICPixelFormat32bppPBGRA;
        frame.SetPixelFormat(&mut format).ok()?;
        frame.WriteSource(&source, std::ptr::null()).ok()?;
        frame.Commit().ok()?;
        encoder.Commit().ok()?;

        let mut end = 0u64;
        stream.Seek(0, STREAM_SEEK_CUR, Some(&mut end)).ok()?;
        stream.Seek(0, STREAM_SEEK_SET, None).ok()?;
        let mut bytes = vec![0u8; end as usize];
        let mut read = 0u32;
        stream.Read(bytes.as_mut_ptr().cast(), end as u32, Some(&mut read)).ok().ok()?;
        bytes.truncate(read as usize);
        (!bytes.is_empty()).then(|| ImageData(bytes.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    #[test]
    fn running_process_icon_becomes_a_png() {
        // SAFETY: COM nesta thread de teste.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
        // O próprio executável dos testes está a correr: tem de haver ícone (o genérico).
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_stem().unwrap().to_string_lossy().into_owned();
        let png = for_app(&name).expect("ícone do processo de teste");
        assert_eq!(&png.0[1..4], b"PNG");
        assert!(for_app("").is_none());
        assert!(for_app("app-que-nao-existe-123").is_none());
    }
}
