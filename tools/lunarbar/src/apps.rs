//! Application discovery for lunarbar's launcher menu — a dependency-free
//! reader for the freedesktop.org (XDG) Desktop Entry + Base Directory specs
//! ("OpenDesktop"). Any program that ships a standard `.desktop` file shows up
//! automatically, exactly as it would in a full desktop's menu.
//!
//! Implemented per spec:
//! - directories: `$XDG_DATA_HOME` (default `~/.local/share`) then each of
//!   `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`), each + `/applications`,
//!   recursed; earlier dirs win by desktop-file ID (so a user override shadows
//!   the system copy).
//! - only `Type=Application` entries, skipping `NoDisplay=true` / `Hidden=true`.
//! - `OnlyShowIn` / `NotShowIn` honoured against `$XDG_CURRENT_DESKTOP`.
//! - `TryExec` must resolve on `PATH` (or as an absolute path) or the entry is
//!   dropped, so dead menu items never appear.
//! - `Exec` field codes (`%f`, `%U`, …) stripped; `Terminal=true` entries are
//!   wrapped in the eclipse-terminal command.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Hard ceiling on menu entries (and walk budget) so opening the launcher
/// cannot stall the Wayland event loop on a pathological applications tree.
const MAX_APPS: usize = 2_048;
const MAX_WALK: usize = 8_000;
const MAX_DEPTH: u32 = 8;
const MAX_DESKTOP_BYTES: u64 = 256 * 1024;

pub struct AppEntry {
    pub name: String,
    pub exec: String,
    /// `Icon=` value (name or absolute path), resolved by icons::IconCache.
    pub icon: Option<String>,
}

/// Scan the XDG applications directories for launchable entries. `terminal`
/// wraps `Terminal=true` programs (and is the builtin Terminal row's command).
pub fn scan_apps(terminal: &str) -> Vec<AppEntry> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{home}/.local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());

    // Desktops we count as "us" for OnlyShowIn/NotShowIn. labwc is wlroots-
    // based; include the common aliases so entries gated to a generic wlroots
    // or "labwc" desktop still show.
    let current = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let desktops: Vec<String> = current
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect();

    let mut roots = vec![PathBuf::from(&data_home)];
    roots.extend(data_dirs.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));

    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut out: Vec<AppEntry> = Vec::new();
    let mut budget = MAX_WALK;
    for root in roots {
        if out.len() >= MAX_APPS || budget == 0 {
            break;
        }
        let apps_dir = root.join("applications");
        collect_dir(
            &apps_dir,
            terminal,
            &desktops,
            &mut seen_ids,
            &mut out,
            &mut budget,
        );
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Walk `root`, deriving each file's desktop-file ID from its path relative to
/// `root` (subdir separators become '-', per spec). Iterative instead of
/// recursive to avoid deep coroutine call stacks on large application trees.
fn collect_dir(
    root: &Path,
    terminal: &str,
    desktops: &[String],
    seen: &mut HashSet<String>,
    out: &mut Vec<AppEntry>,
    budget: &mut usize,
) {
    // Stack items: (directory, depth).
    let mut stack: Vec<(PathBuf, u32)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH || *budget == 0 || out.len() >= MAX_APPS {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            if *budget == 0 || out.len() >= MAX_APPS {
                return;
            }
            *budget = budget.saturating_sub(1);
            let p = e.path();
            let ft = match e.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push((p, depth + 1));
            } else if p.extension().map(|x| x == "desktop").unwrap_or(false) {
                let id = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('/', "-");
                if !seen.insert(id) {
                    continue; // shadowed by a higher-priority dir
                }
                if let Some(a) = parse_desktop(&p, terminal, desktops) {
                    out.push(a);
                }
            }
        }
    }
}

/// Parse one `.desktop` file into a menu entry, or None if the spec says it
/// should not be shown here.
fn parse_desktop(path: &Path, terminal: &str, desktops: &[String]) -> Option<AppEntry> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_DESKTOP_BYTES {
        return None;
    }
    let s = std::fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let (mut name, mut name_loc, mut exec, mut try_exec) = (None, None, None, None);
    let mut icon = None;
    let (mut is_app, mut hidden, mut wants_term) = (false, false, false);
    let (mut only_show, mut not_show) = (Vec::new(), Vec::new());

    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]"; // ignore actions/other groups
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Name" => name = Some(v.to_string()),
            "Type" => is_app = v == "Application",
            "Exec" => exec = Some(v.to_string()),
            "Icon" if !v.is_empty() => icon = Some(v.to_string()),
            "TryExec" => try_exec = Some(v.to_string()),
            "NoDisplay" | "Hidden" if v == "true" => hidden = true,
            "Terminal" if v == "true" => wants_term = true,
            "OnlyShowIn" => only_show = split_list(v),
            "NotShowIn" => not_show = split_list(v),
            _ => {
                // Localized name for our locale's language, e.g. Name[es].
                if k.starts_with("Name[") && name_loc.is_none() && locale_matches(k) {
                    name_loc = Some(v.to_string());
                }
            }
        }
    }

    if !is_app || hidden {
        return None;
    }
    // OnlyShowIn / NotShowIn gating against the current desktop(s).
    if !only_show.is_empty() && !only_show.iter().any(|d| desktops.contains(d)) {
        return None;
    }
    if not_show.iter().any(|d| desktops.contains(d)) {
        return None;
    }
    // TryExec: the named binary must exist, else the entry is a dead link.
    if let Some(te) = try_exec {
        if !executable_exists(&te) {
            return None;
        }
    }

    let name = name_loc.or(name)?;
    let exec = strip_field_codes(&exec?);
    if exec.is_empty() {
        return None;
    }
    let exec = if wants_term {
        format!("{terminal} {exec}")
    } else {
        exec
    };
    Some(AppEntry { name, exec, icon })
}

/// `OnlyShowIn`/`NotShowIn` are ';'-separated, lowercased for comparison.
fn split_list(v: &str) -> Vec<String> {
    v.split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// Does a `Name[xx]` / `Name[xx_YY]` key match `$LANG`'s language?
fn locale_matches(key: &str) -> bool {
    let Some(tag) = key.strip_prefix("Name[").and_then(|s| s.strip_suffix(']')) else {
        return false;
    };
    let lang = std::env::var("LANG").unwrap_or_default();
    let lang = lang.split(['.', '_']).next().unwrap_or("");
    !lang.is_empty() && tag.split('_').next() == Some(lang)
}

/// Remove XDG Exec field codes (`%f %F %u %U %i %c %k` …); collapse whitespace.
fn strip_field_codes(exec: &str) -> String {
    let mut out = exec.to_string();
    for code in [
        "%U", "%u", "%F", "%f", "%i", "%c", "%k", "%d", "%D", "%n", "%N", "%v", "%m",
    ] {
        out = out.replace(code, "");
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Search-normalise a string: lowercase with Latin-1 diacritics stripped, so
/// typing "configuracion" in the launcher filter matches "Configuración".
pub fn norm_key(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

/// True if `cmd` (absolute, or a bare name on `PATH`) is an executable file.
fn executable_exists(cmd: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let is_exec = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if cmd.contains('/') {
        return is_exec(Path::new(cmd));
    }
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
    path.split(':').any(|d| is_exec(&Path::new(d).join(cmd)))
}
