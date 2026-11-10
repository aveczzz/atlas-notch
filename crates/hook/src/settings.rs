//! `island-hook settings install|uninstall`: junta (ou tira) os hooks e a linha de estado da
//! island ao `~/.claude/settings.json` do utilizador.
//!
//! Porque é feito aqui, em Rust, e não no `.ps1`: o `ConvertTo-Json` do Windows PowerShell 5.1
//! trunca a profundidade por omissão, escapa `<`, `>`, `&` e `'` e reformata o ficheiro todo.
//! O `serde_json` (com `preserve_order`) mantém a ordem das chaves e o resto do ficheiro como
//! estava, e isto testa-se com `cargo test`. O `instalar.ps1` só chama estes subcomandos.
//!
//! Regras:
//! - **Idempotente**: correr duas vezes não muda nada (nem escreve, nem faz backup).
//! - **Só toca no que é nosso**: um comando é nosso se o executável se chama `island-hook`.
//!   Hooks e `statusLine` de outras ferramentas ficam como estão.
//! - Se já houver um `statusLine` que não é nosso, **não se substitui** sem `--replace-statusline`
//!   (o instalador pergunta antes). O script antigo de teste (`island-statusline.ps1`) migra sozinho.
//! - Antes de gravar, backup `settings.json.island-backup-<unix>`; a gravação é atómica.
//! - JSON inválido: aborta sem tocar em nada.

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Eventos que a island ouve e o `matcher` de cada um (só os de ferramentas o usam).
const EVENTS: [(&str, Option<&str>); 8] = [
    ("SessionStart", None),
    ("UserPromptSubmit", None),
    ("PreToolUse", Some("*")),
    ("PostToolUse", Some("*")),
    ("Notification", None),
    ("Stop", None),
    ("SubagentStop", None),
    ("SessionEnd", None),
];
const HOOK_TIMEOUT_SECS: u64 = 2;
/// O script de teste que a island teve antes de a linha de estado passar para o `island-hook`.
const LEGACY_SCRIPT: &str = "island-statusline.ps1";

/// O que aconteceu à linha de estado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusLine {
    Set,
    Updated,
    Unchanged,
    /// Era o script antigo de teste da island.
    Migrated,
    /// Havia outra (do utilizador) e foi substituída por pedido (`--replace-statusline`).
    Replaced,
    /// Há outra e ficou como estava. Traz o comando.
    KeptForeign(String),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub hooks_added: usize,
    pub hooks_updated: usize,
    pub hooks_unchanged: usize,
    pub statusline: Option<StatusLine>,
}

impl Report {
    fn changed(&self) -> bool {
        self.hooks_added > 0
            || self.hooks_updated > 0
            || !matches!(self.statusline, Some(StatusLine::Unchanged | StatusLine::KeptForeign(_)) | None)
    }
}

/// `C:\Users\x y\island-hook.exe` -> `"C:/Users/x y/island-hook.exe"`: barras normais (o Claude
/// Code corre os hooks no Git Bash, onde `\` é escape) e aspas só se houver espaços.
pub fn command_path(exe: &Path) -> String {
    let p = exe.to_string_lossy().replace('\\', "/");
    // O prefixo `\\?\` que o Windows às vezes põe nos caminhos absolutos não serve ao Git Bash.
    let p = p.strip_prefix("//?/").unwrap_or(&p).to_owned();
    if p.contains(' ') { format!("\"{p}\"") } else { p }
}

/// O primeiro "elemento" de um comando (o executável), com ou sem aspas.
fn executable(command: &str) -> &str {
    let c = command.trim_start();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest);
    }
    c.split_whitespace().next().unwrap_or("")
}

/// O comando é do `island-hook` (qualquer pasta, com ou sem `.exe`)?
fn is_ours(command: &str) -> bool {
    let exe = executable(command).replace('\\', "/").to_ascii_lowercase();
    matches!(exe.rsplit('/').next(), Some("island-hook.exe" | "island-hook"))
}

fn is_legacy_statusline(command: &str) -> bool {
    command.to_ascii_lowercase().contains(LEGACY_SCRIPT)
}

fn hook_entry(matcher: Option<&str>, command: &str) -> Value {
    let mut group = Map::new();
    if let Some(m) = matcher {
        group.insert("matcher".into(), json!(m));
    }
    group.insert("hooks".into(), json!([{ "type": "command", "command": command, "timeout": HOOK_TIMEOUT_SECS }]));
    Value::Object(group)
}

/// Os comandos `island-hook` de um grupo (`{"matcher":..., "hooks":[...]}`), por referência.
fn our_commands(group: &mut Value) -> Vec<&mut Value> {
    let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else { return Vec::new() };
    hooks.iter_mut().filter(|h| h.get("command").and_then(Value::as_str).is_some_and(is_ours)).collect()
}

/// Junta os hooks e a linha de estado ao `settings` (já lido). `exe`: o `island-hook.exe` final.
pub fn merge(settings: &mut Value, exe: &Path, replace_foreign: bool) -> Result<Report, String> {
    let root = settings.as_object_mut().ok_or("o settings.json não é um objeto JSON ({...})")?;
    let hook_cmd = command_path(exe);
    let status_cmd = format!("{hook_cmd} statusline");
    let mut report = Report::default();

    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("a chave \"hooks\" do settings.json não é um objeto")?;
    for (event, matcher) in EVENTS {
        let groups = hooks.entry(event).or_insert_with(|| json!([]));
        let groups = groups.as_array_mut().ok_or_else(|| format!("hooks.{event} não é uma lista"))?;
        let mut found = false;
        for group in groups.iter_mut() {
            for h in our_commands(group) {
                found = true;
                if h.get("command").and_then(Value::as_str) == Some(hook_cmd.as_str()) {
                    report.hooks_unchanged += 1;
                } else {
                    h["command"] = json!(hook_cmd);
                    report.hooks_updated += 1;
                }
            }
        }
        if !found {
            groups.push(hook_entry(matcher, &hook_cmd));
            report.hooks_added += 1;
        }
    }

    let new_status = json!({ "type": "command", "command": status_cmd });
    report.statusline = Some(match root.get("statusLine") {
        None | Some(Value::Null) => {
            root.insert("statusLine".into(), new_status);
            StatusLine::Set
        }
        Some(current) => {
            let cmd = current.get("command").and_then(Value::as_str).unwrap_or("").to_owned();
            if is_ours(&cmd) {
                if cmd == status_cmd {
                    StatusLine::Unchanged
                } else {
                    root["statusLine"]["command"] = json!(status_cmd);
                    StatusLine::Updated
                }
            } else if is_legacy_statusline(&cmd) {
                root.insert("statusLine".into(), new_status);
                StatusLine::Migrated
            } else if replace_foreign {
                root.insert("statusLine".into(), new_status);
                StatusLine::Replaced
            } else {
                let shown = if cmd.is_empty() { current.to_string() } else { cmd };
                StatusLine::KeptForeign(shown)
            }
        }
    });
    Ok(report)
}

/// Tira os hooks e a linha de estado da island (e o script antigo). Devolve quantos comandos tirou.
pub fn remove(settings: &mut Value) -> Result<usize, String> {
    let root = settings.as_object_mut().ok_or("o settings.json não é um objeto JSON ({...})")?;
    let mut removed = 0;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        for (_, groups) in hooks.iter_mut() {
            let Some(groups) = groups.as_array_mut() else { continue };
            for group in groups.iter_mut() {
                if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    let before = list.len();
                    list.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(is_ours));
                    removed += before - list.len();
                }
            }
            // Um grupo que ficou sem hooks era só nosso: sai.
            groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|l| !l.is_empty()));
        }
        hooks.retain(|_, v| v.as_array().is_none_or(|l| !l.is_empty()));
    }
    if root.get("hooks").and_then(Value::as_object).is_some_and(Map::is_empty) {
        root.remove("hooks");
    }
    let ours_status = root
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str)
        .is_some_and(|c| is_ours(c) || is_legacy_statusline(c));
    if ours_status {
        root.remove("statusLine");
        removed += 1;
    }
    Ok(removed)
}

/// `%USERPROFILE%\.claude\settings.json`.
fn default_settings_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".claude").join("settings.json"))
}

/// Lê o settings (`{}` se o ficheiro não existe). Erro se não for JSON válido.
fn read(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let text = text.trim_start_matches('\u{feff}');
            if text.trim().is_empty() {
                return Ok(json!({}));
            }
            serde_json::from_str(text).map_err(|e| format!("{} não é JSON válido ({e}); não foi alterado", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("não consegui ler {}: {e}", path.display())),
    }
}

/// Backup ao lado do ficheiro e gravação atómica (temporário + rename).
fn write(path: &Path, value: &Value) -> Result<Option<PathBuf>, String> {
    let dir = path.parent().ok_or("caminho sem pasta")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("não consegui criar {}: {e}", dir.display()))?;
    let mut backup = None;
    if path.exists() {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let name = format!("{}.island-backup-{secs}", path.file_name().map_or_else(|| "settings.json".into(), |n| n.to_string_lossy()));
        let dest = dir.join(name);
        std::fs::copy(path, &dest).map_err(|e| format!("não consegui fazer o backup: {e}"))?;
        backup = Some(dest);
    }
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    let tmp = path.with_extension("json.island-tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("não consegui escrever {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("não consegui gravar {}: {e}", path.display())
    })?;
    Ok(backup)
}

struct Options {
    settings: PathBuf,
    exe: PathBuf,
    replace_statusline: bool,
    dry_run: bool,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut settings = None;
    let mut exe = None;
    let (mut replace_statusline, mut dry_run) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--settings" => settings = Some(PathBuf::from(it.next().ok_or("falta o valor de --settings")?)),
            "--exe" => exe = Some(PathBuf::from(it.next().ok_or("falta o valor de --exe")?)),
            "--replace-statusline" => replace_statusline = true,
            "--dry-run" => dry_run = true,
            other => return Err(format!("opção desconhecida: {other}")),
        }
    }
    Ok(Options {
        settings: settings.or_else(default_settings_path).ok_or("não sei onde está o ~/.claude (USERPROFILE em falta)")?,
        exe: match exe {
            Some(e) => e,
            None => std::env::current_exe().map_err(|e| format!("não sei onde está o island-hook.exe: {e}"))?,
        },
        replace_statusline,
        dry_run,
    })
}

/// `island-hook settings <install|uninstall> [opções]`. Devolve o código de saída (0 = ok,
/// 2 = erro). A saída é `chave=valor`, uma por linha, para o `.ps1` ler.
pub fn cli(args: &[String]) -> i32 {
    match run(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("erro: {e}");
            2
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (action, rest) = args.split_first().ok_or("uso: island-hook settings <install|uninstall> [--settings F] [--exe F] [--replace-statusline] [--dry-run]")?;
    let opts = parse_options(rest)?;
    let mut settings = read(&opts.settings)?;
    println!("settings={}", opts.settings.display());
    match action.as_str() {
        "install" => {
            let exe = std::fs::canonicalize(&opts.exe).unwrap_or(opts.exe.clone());
            let report = merge(&mut settings, &exe, opts.replace_statusline)?;
            println!("hooks_adicionados={}", report.hooks_added);
            println!("hooks_atualizados={}", report.hooks_updated);
            println!("hooks_iguais={}", report.hooks_unchanged);
            match &report.statusline {
                Some(StatusLine::KeptForeign(cmd)) => println!("statusline=outra\nstatusline_comando={cmd}"),
                Some(s) => println!("statusline={}", match s {
                    StatusLine::Set => "definida",
                    StatusLine::Updated => "atualizada",
                    StatusLine::Unchanged => "igual",
                    StatusLine::Migrated => "migrada",
                    StatusLine::Replaced => "substituida",
                    StatusLine::KeptForeign(_) => unreachable!(),
                }),
                None => {}
            }
            finish(&opts, &settings, report.changed())
        }
        "uninstall" => {
            let removed = remove(&mut settings)?;
            println!("removidos={removed}");
            finish(&opts, &settings, removed > 0)
        }
        other => Err(format!("ação desconhecida: {other}")),
    }
}

fn finish(opts: &Options, settings: &Value, changed: bool) -> Result<(), String> {
    if !changed {
        println!("resultado=sem-alteracoes");
    } else if opts.dry_run {
        println!("resultado=simulacao");
    } else {
        if let Some(b) = write(&opts.settings, settings)? {
            println!("backup={}", b.display());
        }
        println!("resultado=gravado");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Users\ana\AppData\Local\dynamic-island\island-hook.exe")
    }

    const CMD: &str = "C:/Users/ana/AppData/Local/dynamic-island/island-hook.exe";

    #[test]
    fn installs_into_an_empty_settings() {
        let mut s = json!({});
        let r = merge(&mut s, &exe(), false).unwrap();
        assert_eq!((r.hooks_added, r.hooks_updated, r.hooks_unchanged), (8, 0, 0));
        assert_eq!(r.statusline, Some(StatusLine::Set));
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0], json!({"type": "command", "command": CMD, "timeout": 2}));
        assert_eq!(s["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert!(s["hooks"]["Stop"][0].get("matcher").is_none());
        assert_eq!(s["statusLine"], json!({"type": "command", "command": format!("{CMD} statusline")}));
    }

    #[test]
    fn running_it_twice_changes_nothing() {
        let mut s = json!({"model": "opus"});
        merge(&mut s, &exe(), false).unwrap();
        let after_first = s.clone();
        let r = merge(&mut s, &exe(), false).unwrap();
        assert_eq!(s, after_first);
        assert!(!r.changed());
        assert_eq!((r.hooks_added, r.hooks_updated, r.hooks_unchanged), (0, 0, 8));
        assert_eq!(r.statusline, Some(StatusLine::Unchanged));
    }

    #[test]
    fn other_keys_hooks_and_order_are_kept() {
        let mut s = json!({
            "theme": "dark",
            "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": "notify-send acabou"}]}],
                "PreCompact": [{"hooks": [{"type": "command", "command": "x"}]}]
            },
            "model": "opus"
        });
        merge(&mut s, &exe(), false).unwrap();
        let keys: Vec<_> = s.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["theme", "hooks", "model", "statusLine"], "a ordem das chaves não muda");
        // O hook do utilizador fica, e o nosso entra a seguir, em grupo à parte.
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], "notify-send acabou");
        assert_eq!(s["hooks"]["Stop"][1]["hooks"][0]["command"], CMD);
        assert_eq!(s["hooks"]["PreCompact"][0]["hooks"][0]["command"], "x");
    }

    #[test]
    fn a_moved_install_updates_the_path_in_place() {
        let mut s = json!({});
        merge(&mut s, Path::new(r"C:\old\place\island-hook.exe"), false).unwrap();
        let r = merge(&mut s, &exe(), false).unwrap();
        assert_eq!((r.hooks_added, r.hooks_updated), (0, 8));
        assert_eq!(r.statusline, Some(StatusLine::Updated));
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 1, "não duplica");
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], CMD);
    }

    #[test]
    fn paths_with_spaces_are_quoted_and_still_recognised() {
        let spaced = PathBuf::from(r"C:\Users\Ana Maria\AppData\Local\dynamic-island\island-hook.exe");
        assert_eq!(command_path(&spaced), "\"C:/Users/Ana Maria/AppData/Local/dynamic-island/island-hook.exe\"");
        let mut s = json!({});
        merge(&mut s, &spaced, false).unwrap();
        let r = merge(&mut s, &spaced, false).unwrap();
        assert!(!r.changed(), "o comando entre aspas é reconhecido como nosso");
        assert_eq!(command_path(Path::new(r"\\?\C:\x\island-hook.exe")), "C:/x/island-hook.exe");
    }

    #[test]
    fn a_foreign_statusline_is_kept_unless_asked() {
        let mut s = json!({"statusLine": {"type": "command", "command": "ccstatusline"}});
        let r = merge(&mut s, &exe(), false).unwrap();
        assert_eq!(r.statusline, Some(StatusLine::KeptForeign("ccstatusline".into())));
        assert_eq!(s["statusLine"]["command"], "ccstatusline");
        // Os hooks entram na mesma.
        assert_eq!(r.hooks_added, 8);

        let r = merge(&mut s, &exe(), true).unwrap();
        assert_eq!(r.statusline, Some(StatusLine::Replaced));
        assert_eq!(s["statusLine"]["command"], format!("{CMD} statusline"));
    }

    #[test]
    fn the_old_test_script_is_migrated_without_asking() {
        let mut s = json!({"statusLine": {"type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:/Users/ana/AppData/Local/dynamic-island/island-statusline.ps1"}});
        let r = merge(&mut s, &exe(), false).unwrap();
        assert_eq!(r.statusline, Some(StatusLine::Migrated));
        assert_eq!(s["statusLine"]["command"], format!("{CMD} statusline"));
    }

    #[test]
    fn uninstall_removes_only_ours_and_restores_the_rest() {
        let original = json!({
            "theme": "dark",
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send acabou"}]}]}
        });
        let mut s = original.clone();
        merge(&mut s, &exe(), false).unwrap();
        let removed = remove(&mut s).unwrap();
        assert_eq!(removed, 9, "8 hooks e a linha de estado");
        assert_eq!(s, original, "fica exatamente como estava antes");
        assert_eq!(remove(&mut s).unwrap(), 0, "e uma segunda vez não faz nada");
    }

    #[test]
    fn uninstall_leaves_a_foreign_statusline_and_drops_empty_hooks() {
        let mut s = json!({"statusLine": {"type": "command", "command": "ccstatusline"}});
        merge(&mut s, &exe(), false).unwrap();
        remove(&mut s).unwrap();
        assert_eq!(s, json!({"statusLine": {"type": "command", "command": "ccstatusline"}}));
        assert!(s.get("hooks").is_none(), "sem hooks de ninguém, a chave sai");
    }

    #[test]
    fn broken_shapes_are_reported_not_overwritten() {
        assert!(merge(&mut json!([]), &exe(), false).is_err());
        assert!(merge(&mut json!({"hooks": []}), &exe(), false).is_err());
        assert!(merge(&mut json!({"hooks": {"Stop": {}}}), &exe(), false).is_err());
    }

    #[test]
    fn recognises_our_commands_only() {
        assert!(is_ours("C:/x/island-hook.exe"));
        assert!(is_ours("\"C:/a b/Island-Hook.EXE\" statusline"));
        assert!(is_ours("island-hook"));
        assert!(!is_ours("C:/x/island-hook-other.exe"));
        assert!(!is_ours("ccstatusline"));
        assert!(!is_ours("echo island-hook.exe"));
    }

    fn temp_settings(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("island-settings-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(".claude").join("settings.json")
    }

    #[test]
    fn creates_the_file_and_folder_when_missing_then_backs_up_on_change() {
        let path = temp_settings("create");
        let args = |extra: &[&str]| -> Vec<String> {
            ["install", "--settings", path.to_str().unwrap(), "--exe", r"C:\x\island-hook.exe"].iter().chain(extra).map(|s| s.to_string()).collect()
        };
        assert_eq!(cli(&args(&[])), 0);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n') && text.contains("\"statusLine\""));
        let dir = path.parent().unwrap();
        let backups = |dir: &Path| std::fs::read_dir(dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains("island-backup")).count();
        assert_eq!(backups(dir), 0, "ficheiro novo: nada para guardar");
        // Segunda vez: igual, não escreve nem faz backup.
        assert_eq!(cli(&args(&[])), 0);
        assert_eq!(backups(dir), 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        // Uma mudança de caminho já guarda o que lá estava.
        let moved: Vec<String> = ["install", "--settings", path.to_str().unwrap(), "--exe", r"C:\outro\island-hook.exe"].iter().map(|s| s.to_string()).collect();
        assert_eq!(cli(&moved), 0);
        assert_eq!(backups(dir), 1);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn invalid_json_is_left_untouched() {
        let path = temp_settings("invalid");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ \"model\": \"opus\" \"theme\": 1 }").unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let args: Vec<String> = ["install", "--settings", path.to_str().unwrap(), "--exe", r"C:\x\island-hook.exe"].iter().map(|s| s.to_string()).collect();
        assert_eq!(cli(&args), 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn dry_run_writes_nothing() {
        let path = temp_settings("dry");
        let args: Vec<String> = ["install", "--dry-run", "--settings", path.to_str().unwrap(), "--exe", r"C:\x\island-hook.exe"].iter().map(|s| s.to_string()).collect();
        assert_eq!(cli(&args), 0);
        assert!(!path.exists());
    }
}
