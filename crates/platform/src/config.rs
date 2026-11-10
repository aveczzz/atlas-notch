//! Configuração do utilizador (`config.toml`) e estado que sobrevive a reiniciar (`tray.txt`).
//!
//! Tudo em `%APPDATA%\dynamic-island`. O ficheiro de configuração é um TOML com chaves
//! `true`/`false`. Um leitor pequeno chega para isso e poupa um crate inteiro de TOML.
//! Chaves desconhecidas ou mal escritas são ignoradas com um aviso: uma gralha não pode
//! impedir a island de arrancar.

use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Arrancar com o Windows (só para este utilizador, só a cópia instalada).
    pub autostart: bool,
    /// Esconder quando uma app ocupa o ecrã inteiro do monitor da island.
    pub hide_in_fullscreen: bool,
    /// Guardar a lista da Tray entre arranques.
    pub keep_tray: bool,
    /// Avisos do sistema (teclas de bloqueio, carregador, dispositivo de áudio, copiar).
    pub system_alerts: bool,
    /// Vidro translúcido quando a island cresce; desligado, preto quase opaco.
    pub glass: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { autostart: true, hide_in_fullscreen: true, keep_tray: true, glass: true, system_alerts: true }
    }
}

/// O ficheiro que o menu "Abrir configuração" cria e abre quando ainda não existe.
pub const DEFAULT_FILE: &str = "\
# Dynamic Island: configuração.
# Grava o ficheiro e a island aplica as mudanças logo, sem reiniciar.

# Arrancar com o Windows.
arrancar_com_windows = true

# Esconder a island quando uma app (jogo, vídeo, apresentação) ocupa o ecrã inteiro.
esconder_em_ecra_inteiro = true

# Lembrar os ficheiros da Tray depois de fechar a island.
guardar_tray = true

# Vidro translúcido quando a island cresce. Desligado, fica preto quase opaco.
vidro = true

# Avisos curtos: Caps/Num/Scroll Lock, carregador e bateria, dispositivo de áudio e Copiado.
avisos_do_sistema = true
";

impl Config {
    /// Lê o texto do ficheiro. Devolve também os avisos (linhas que não percebeu).
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut cfg = Self::default();
        let mut warnings = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("linha {}: falta o '='", n + 1));
                continue;
            };
            let value = match value.trim() {
                "true" => true,
                "false" => false,
                other => {
                    warnings.push(format!("linha {}: '{other}' não é true nem false", n + 1));
                    continue;
                }
            };
            match key.trim() {
                "arrancar_com_windows" => cfg.autostart = value,
                "esconder_em_ecra_inteiro" => cfg.hide_in_fullscreen = value,
                "guardar_tray" => cfg.keep_tray = value,
                "vidro" => cfg.glass = value,
                "avisos_do_sistema" => cfg.system_alerts = value,
                other => warnings.push(format!("linha {}: chave desconhecida '{other}'", n + 1)),
            }
        }
        (cfg, warnings)
    }

    /// Lê `config.toml`. Se não existir, valem os valores por omissão.
    pub fn load(dir: &Path) -> Self {
        match fs::read_to_string(dir.join(CONFIG_FILE)) {
            Ok(text) => {
                let (cfg, warnings) = Self::parse(text.trim_start_matches('\u{feff}'));
                for w in warnings {
                    log!("config.toml, {w}");
                }
                cfg
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                log!("config.toml: {e}");
                Self::default()
            }
        }
    }
}

pub const CONFIG_FILE: &str = "config.toml";
const TRAY_FILE: &str = "tray.txt";

/// `%APPDATA%\dynamic-island`, criada se faltar.
pub fn dir() -> io::Result<PathBuf> {
    let base = std::env::var_os("APPDATA").ok_or_else(|| io::Error::other("APPDATA não definida"))?;
    let dir = PathBuf::from(base).join("dynamic-island");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Caminho do `config.toml`, criado com os valores por omissão se ainda não existir.
pub fn ensure_file(dir: &Path) -> io::Result<PathBuf> {
    let path = dir.join(CONFIG_FILE);
    if !path.exists() {
        fs::write(&path, DEFAULT_FILE)?;
    }
    Ok(path)
}

/// Os ficheiros da Tray guardados, mais recente primeiro. Os que já não existem ficam de fora.
pub fn load_tray(dir: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(dir.join(TRAY_FILE)) else { return Vec::new() };
    parse_tray(&text).into_iter().filter(|p| Path::new(p).exists()).collect()
}

fn parse_tray(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()
}

/// Um caminho por linha: os nomes de ficheiro no Windows não têm mudanças de linha.
/// Escreve para um temporário e troca, para um corte de energia não deixar meio ficheiro.
pub fn save_tray(dir: &Path, paths: &[String]) -> io::Result<()> {
    let tmp = dir.join("tray.txt.tmp");
    let mut text = paths.join("\n");
    text.push('\n');
    fs::write(&tmp, text)?;
    fs::rename(&tmp, dir.join(TRAY_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_file_gives_the_defaults() {
        let (cfg, warnings) = Config::parse(DEFAULT_FILE);
        assert_eq!(cfg, Config::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn reads_values_and_comments() {
        let (cfg, warnings) = Config::parse("arrancar_com_windows = false # não\n  guardar_tray=false\n");
        assert_eq!(cfg, Config { autostart: false, hide_in_fullscreen: true, keep_tray: false, glass: true, system_alerts: true });
        assert!(warnings.is_empty());
    }

    #[test]
    fn typos_warn_and_keep_the_default() {
        let (cfg, warnings) = Config::parse("arrancar_com_windows = talvez\nguardar_trey = false\nlixo\n");
        assert_eq!(cfg, Config::default());
        assert_eq!(warnings.len(), 3);
    }

    #[test]
    fn tray_roundtrip() {
        let dir = std::env::temp_dir().join(format!("island-tray-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("a b.txt");
        fs::write(&existing, "x").unwrap();
        let paths = vec![existing.to_string_lossy().into_owned(), dir.join("apagado.txt").to_string_lossy().into_owned()];
        save_tray(&dir, &paths).unwrap();
        assert_eq!(parse_tray(&fs::read_to_string(dir.join(TRAY_FILE)).unwrap()), paths);
        // O que já não existe sai ao carregar.
        assert_eq!(load_tray(&dir), paths[..1]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
