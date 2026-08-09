//! XDG icon lookup + raster cache for lunarbar's taskbar buttons and app
//! menu — dependency-light: tiny-skia's png-format feature decodes the PNGs
//! (pure Rust, the binary stays static), no librsvg/gdk-pixbuf. SVG/XPM-only
//! themes are skipped; any name without a PNG on disk falls back to the
//! letter badge the drawing layer provides.
//!
//! Resolution order for a name (an `Icon=` value or a Wayland app_id):
//! 1. an absolute path is loaded directly;
//! 2. the name (lowercased, `.png` suffix and reverse-DNS prefix stripped)
//!    against an index of every PNG under `$XDG_DATA_HOME/icons`,
//!    `$XDG_DATA_DIRS/*/icons` and `/usr/share/pixmaps`, preferring sizes
//!    near 48px and `apps/` categories — a pragmatic stand-in for full
//!    index.theme parsing that behaves identically on well-formed themes;
//! 3. the app_id against the `Icon=` of the .desktop file of the same stem
//!    (org.mozilla.firefox → firefox.desktop's icon), like real taskbars.
//!
//! Decoded icons are scaled once (bilinear, aspect preserved, centred) to the
//! requested slot size and cached; misses are cached too, so a window whose
//! app has no icon costs one lookup, not one per frame.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

/// Decoded (name, size) entries kept in RAM. A long session that opens many
/// distinct app_ids must not grow this without bound (amplifies heap pressure
/// that already stresses the kernel EventBus path on Eclipse).
const MAX_ICON_CACHE_ENTRIES: usize = 256;

/// Refuse to decode PNGs larger than this (raw file bytes) — protects the
/// event-loop thread from decompress bombs / multi‑MB wallpapers used as Icon=.
const MAX_PNG_BYTES: u64 = 4 * 1024 * 1024;

/// Refuse source pixmaps larger than this on either edge before scaling.
const MAX_PNG_DIM: u32 = 4096;

#[derive(Default)]
pub struct IconCache {
    /// Nested so a hit can look up by `&str` without allocating a `String` key.
    cache: HashMap<String, HashMap<u32, Option<Rc<Pixmap>>>>,
    /// Insertion order for approximate LRU eviction (front = oldest).
    order: VecDeque<(String, u32)>,
    /// Lowercased file stem → (score, path) of the best on-disk PNG.
    index: Option<HashMap<String, (i32, PathBuf)>>,
    /// Lowercased .desktop stem → its Icon= value.
    desktop: Option<HashMap<String, String>>,
}

impl IconCache {
    /// Entries currently held (hits and cached misses).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Build the on-disk indexes up front. Both are built lazily on first
    /// lookup otherwise — and that first lookup happens inside a render pass,
    /// on the same thread that services pointer, keyboard and configure
    /// events, so a cold-cache walk of thousands of theme files would freeze
    /// the panel mid-interaction. Called once at startup, before the bars are
    /// mapped, where a pause costs nothing.
    pub fn prewarm(&mut self) {
        self.index.get_or_insert_with(build_index);
        self.desktop.get_or_insert_with(desktop_icon_map);
    }

    /// Resolve `name` to a `size`×`size` pixmap, or None (cached either way).
    pub fn get(&mut self, name: &str, size: u32) -> Option<Rc<Pixmap>> {
        let name = name.trim();
        if name.is_empty() || size == 0 {
            return None;
        }
        if let Some(by_size) = self.cache.get(name) {
            if let Some(hit) = by_size.get(&size) {
                return hit.clone();
            }
        }
        let loaded = self.load(name, size).map(Rc::new);
        while self.order.len() >= MAX_ICON_CACHE_ENTRIES {
            if let Some((old_n, old_s)) = self.order.pop_front() {
                if let Some(m) = self.cache.get_mut(&old_n) {
                    m.remove(&old_s);
                    if m.is_empty() {
                        self.cache.remove(&old_n);
                    }
                }
            } else {
                break;
            }
        }
        self.order.push_back((name.to_string(), size));
        self.cache
            .entry(name.to_string())
            .or_default()
            .insert(size, loaded.clone());
        loaded
    }

    fn load(&mut self, name: &str, size: u32) -> Option<Pixmap> {
        // 1) Absolute path straight from a .desktop Icon= value.
        if name.starts_with('/') {
            return load_scaled(Path::new(name), size);
        }
        // 2) The icon index, under the name's likely stems.
        let index = self.index.get_or_insert_with(build_index);
        for c in name_variants(name) {
            if let Some((_, p)) = index.get(&c) {
                if let Some(pm) = load_scaled(p, size) {
                    return Some(pm);
                }
            }
        }
        // 3) An app_id: follow the matching .desktop file's Icon= (one hop).
        let desktop = self.desktop.get_or_insert_with(desktop_icon_map);
        for c in name_variants(name) {
            if let Some(icon) = desktop.get(&c) {
                let icon = icon.clone();
                if icon.starts_with('/') {
                    return load_scaled(Path::new(&icon), size);
                }
                let index = self.index.get_or_insert_with(build_index);
                for ic in name_variants(&icon) {
                    if let Some((_, p)) = index.get(&ic) {
                        if let Some(pm) = load_scaled(p, size) {
                            return Some(pm);
                        }
                    }
                }
            }
        }
        None
    }
}

/// Lookup keys tried for a name: lowercased (sans .png), then the last
/// dot-segment (org.gnome.Calculator → calculator), the common convention
/// for reverse-DNS app_ids.
fn name_variants(name: &str) -> Vec<String> {
    let base = name.strip_suffix(".png").unwrap_or(name);
    let mut out = vec![base.to_ascii_lowercase()];
    if let Some(tail) = base.rsplit('.').next() {
        let tail = tail.to_ascii_lowercase();
        if !out.contains(&tail) {
            out.push(tail);
        }
    }
    out
}

/// XDG data roots (`$XDG_DATA_HOME`, then each of `$XDG_DATA_DIRS`).
fn data_roots() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{home}/.local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let mut roots = vec![PathBuf::from(&data_home)];
    roots.extend(data_dirs.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    roots
}

/// Walk the icon directories once, keeping the best-scoring PNG per stem.
/// Iterative (explicit stack) instead of recursive so the coroutine call stack
/// stays shallow regardless of how deeply nested the theme tree is.
fn build_index() -> HashMap<String, (i32, PathBuf)> {
    let mut map = HashMap::new();
    let mut roots: Vec<PathBuf> = data_roots().into_iter().map(|r| r.join("icons")).collect();
    roots.push(PathBuf::from("/usr/share/pixmaps"));
    // Bound the walk hard. Every entry visited is a path lookup in the
    // kernel, and `lookup_inode_at` is on the path implicated by
    // docs/README-crash-repro.md — so this runs once at startup and stays
    // small deliberately, trading exhaustive theme coverage (the letter
    // badge covers the misses) for a fraction of the filesystem traffic.
    let mut budget = 4_000usize;
    for root in roots {
        walk(&root, &mut budget, &mut map);
    }
    map
}

fn walk(start: &Path, budget: &mut usize, map: &mut HashMap<String, (i32, PathBuf)>) {
    // Explicit stack instead of recursion: icon trees can be 8+ levels deep;
    // recursive calls would eat that many coroutine stack frames on each icon
    // index build, contributing to the kernel stack-overflow crash.
    let mut stack: Vec<(PathBuf, u32)> = vec![(start.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 8 || *budget == 0 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let p = e.path();
            // is_dir() follows symlinks — icon themes commonly symlink whole size
            // directories; the walk budget bounds any symlink cycle.
            if p.is_dir() {
                // Cursor themes hold hundreds of files that are never app icons.
                if p.file_name().map(|n| n == "cursors").unwrap_or(false) {
                    continue;
                }
                stack.push((p, depth + 1));
            } else if p.extension().map(|x| x == "png").unwrap_or(false) {
                let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let stem = stem.to_ascii_lowercase();
                let sc = score(&p);
                match map.get(&stem) {
                    Some((best, _)) if *best >= sc => {}
                    _ => {
                        map.insert(stem, (sc, p));
                    }
                }
            }
        }
    }
}

/// Prefer sizes near 48px (crisp when downscaled to bar size) and `apps/`
/// categories; unsized locations (pixmaps) sit in the middle.
fn score(p: &Path) -> i32 {
    let mut sc = 500;
    for comp in p.components() {
        let c = comp.as_os_str().to_string_lossy();
        if let Some((a, b)) = c.split_once('x') {
            if let (Ok(n), Ok(_)) = (a.parse::<i32>(), b.parse::<i32>()) {
                sc = 1000 - (n - 48).abs() * 4;
            }
        }
    }
    if p.to_string_lossy().contains("/apps") {
        sc += 50;
    }
    sc
}

/// Map every .desktop stem to its Icon= value (first hit per stem wins,
/// mirroring apps.rs's directory priority).
fn desktop_icon_map() -> HashMap<String, String> {
    let mut map = HashMap::new();
    // Same reasoning as build_index: each entry is a path lookup, and each
    // .desktop file is an open+read+parse.
    let mut budget = 1_500usize;
    for root in data_roots() {
        walk_desktop(&root.join("applications"), &mut budget, &mut map);
    }
    map
}

fn walk_desktop(start: &Path, budget: &mut usize, map: &mut HashMap<String, String>) {
    // Iterative to avoid deep call stacks inside the render path (same
    // rationale as `walk` above).
    let mut stack: Vec<(PathBuf, u32)> = vec![(start.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 4 || *budget == 0 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let p = e.path();
            if p.is_dir() {
                stack.push((p, depth + 1));
            } else if p.extension().map(|x| x == "desktop").unwrap_or(false) {
                let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let stem = stem.to_ascii_lowercase();
                if map.contains_key(&stem) {
                    continue;
                }
                if let Some(icon) = desktop_icon(&p) {
                    map.insert(stem, icon);
                }
            }
        }
    }
}

/// The `Icon=` value of a .desktop file's `[Desktop Entry]` group, if any.
fn desktop_icon(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 256 * 1024 {
        return None;
    }
    let s = std::fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    for line in s.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == "Icon" {
                let v = v.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Decode a PNG and scale it (bilinear, aspect preserved, centred) into a
/// `size`×`size` pixmap.
fn load_scaled(path: &Path, size: u32) -> Option<Pixmap> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_PNG_BYTES {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let src = Pixmap::decode_png(&data).ok()?;
    if src.width() > MAX_PNG_DIM || src.height() > MAX_PNG_DIM {
        return None;
    }
    let (w, h) = (src.width() as f32, src.height() as f32);
    if w < 1.0 || h < 1.0 {
        return None;
    }
    let mut dst = Pixmap::new(size, size)?;
    let s = size as f32 / w.max(h);
    let tx = (size as f32 - w * s) / 2.0;
    let ty = (size as f32 - h * s) / 2.0;
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    dst.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &paint,
        Transform::from_row(s, 0.0, 0.0, s, tx, ty),
        None,
    );
    Some(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_cache_stays_bounded_under_unique_lookups() {
        let mut cache = IconCache::default();
        // Misses are cached too — without a ceiling this would grow forever
        // across a long session of distinct app_ids.
        for i in 0..(MAX_ICON_CACHE_ENTRIES * 3) {
            let _ = cache.get(&format!("no-such-icon-{i}"), 24);
        }
        assert!(
            cache.len() <= MAX_ICON_CACHE_ENTRIES,
            "icon cache must stay ≤ {}, got {}",
            MAX_ICON_CACHE_ENTRIES,
            cache.len()
        );
    }

    #[test]
    fn icon_cache_hit_does_not_grow() {
        let mut cache = IconCache::default();
        let _ = cache.get("missing-a", 24);
        let n = cache.len();
        for _ in 0..100 {
            let _ = cache.get("missing-a", 24);
        }
        assert_eq!(cache.len(), n);
    }
}
