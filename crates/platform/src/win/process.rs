//! Informação sobre outros processos (só leitura, acesso limitado).

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

/// Caminho completo do executável do processo `pid`.
pub fn image_path(pid: u32) -> Option<String> {
    // SAFETY: handle de consulta limitada, fechado no fim; buffer local com o tamanho declarado.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Nome do executável, sem pasta (`code.exe`).
pub fn exe_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Nome para mostrar: o executável sem `.exe`, com a primeira letra maiúscula (`Spotify`).
pub fn display_name(path: &str) -> String {
    let exe = exe_name(path);
    let stem = exe.strip_suffix(".exe").or_else(|| exe.strip_suffix(".EXE")).unwrap_or(exe);
    let mut chars = stem.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_paths() {
        assert_eq!(exe_name(r"C:\Program Files\Spotify\Spotify.exe"), "Spotify.exe");
        assert_eq!(display_name(r"C:\x\chrome.exe"), "Chrome");
        assert_eq!(display_name("msedge"), "Msedge");
        assert_eq!(display_name(""), "");
    }

    #[test]
    fn own_process_has_a_path() {
        let path = image_path(std::process::id()).unwrap();
        assert!(path.to_lowercase().ends_with(".exe"));
    }
}
