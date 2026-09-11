//! Wallpaper backend owned by the settings daemon.
//!
//! Premade packs are scanned from the system wallpapers directory
//! (`/System/User/Wallpapers`, `TONTOO_WALLPAPERS_DIR` override).
//! User customs live in `~/Library/Preferences/com.tontoo.wallpaper/`
//! (`SETTINGS_WALLPAPER_DIR` override): each upload is decoded and stored
//! as PNG, registered in CoreData (`CustomWallpaper` entity) and mirrored
//! in `storage.fico` next to the files. The current wallpaper and the fill
//! mode persist in the `wallpaper` store domain.
//!
//! Socket ops: `wallpaper_get` is a public read op; `wallpaper_set_current`,
//! `wallpaper_set_fill` and `wallpaper_add` are private write ops with no
//! public client library, reserved for the Settings app
//! (`com.tontoo.systemsettings`).
//!
//! Nothing here applies the wallpaper to the desktop; that stays a later
//! step (wallpaper engine). Display only for now.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::store::SettingsStore;

/// System directory holding one premade pack per subdirectory.
pub const DEFAULT_PREMADE_DIR: &str = "/System/User/Wallpapers";
/// Directory name for the user custom wallpapers below `~/Library/Preferences`.
pub const CUSTOM_DIR_NAME: &str = "com.tontoo.wallpaper";
/// Registry mirror next to the custom files.
pub const STORAGE_FICO: &str = "storage.fico";

/// Pack selected when nothing is configured yet (fresh installs boot it).
pub const DEFAULT_PACK: &str = "THAOELAKE";

/// Store domain holding the current wallpaper and the fill mode.
pub const DOMAIN: &str = "wallpaper";
pub const KEY_CURRENT_KIND: &str = "current_kind";
pub const KEY_CURRENT_ID: &str = "current_id";
pub const KEY_FILL: &str = "fill";

pub const OP_WALLPAPER_GET: &str = "wallpaper_get";
pub const OP_WALLPAPER_SET_CURRENT: &str = "wallpaper_set_current";
pub const OP_WALLPAPER_SET_FILL: &str = "wallpaper_set_fill";
pub const OP_WALLPAPER_ADD: &str = "wallpaper_add";
pub const OP_WALLPAPER_APPLY: &str = "wallpaper_apply";
pub const OP_WALLPAPER_DELETE: &str = "wallpaper_delete";

/// Kind tag for premade pack entries.
pub const KIND_PREMADE: &str = "premade";
/// Kind tag for user custom entries.
pub const KIND_CUSTOM: &str = "custom";

/// Wallpaper variants accepted by `wallpaper_apply`.
pub const VARIANTS: &[&str] = &["light", "dark", "auto"];

/// Compositor settings socket: forwards the resolved file for the
/// desktop crossfade (`COMPOSITOR_SOCKET` override).
pub const DEFAULT_COMPOSITOR_SOCKET: &str = "/run/tontoo-compositor.sock";

/// Fill modes accepted by `wallpaper_set_fill`.
pub const FILL_MODES: &[&str] = &["fill", "fit", "stretch", "center", "tile"];
pub const DEFAULT_FILL: &str = "fill";

/// CoreData entity holding one user custom wallpaper per object.
pub const CUSTOM_WALLPAPER_ENTITY: &str = "CustomWallpaper";

/// Premade packs in macOS release order (newest first, unversioned last).
/// Unknown pack ids sort after every known pack, by display name.
pub const PREMADE_ORDER: &[&str] = &[
  "GOLDENGATE",
  "THAOE",
  "THAOELAKE",
  "SEQUOIA",
  "SONOMA",
  "VENTURA",
  "MONTEREY",
  "BIGSUR",
  "CATALINA",
  "MOJAVE",
  "FLOW",
  "TONTOOOS",
];

/// Sort rank of a premade pack id (unknown ids rank last).
pub fn premade_rank(id: &str) -> usize {
  PREMADE_ORDER
    .iter()
    .position(|known| *known == id)
    .unwrap_or(PREMADE_ORDER.len())
}

/// One listed wallpaper: a premade pack or a user custom file.
/// `path` is the light (default) image, `path_dark` the dark variant
/// (same file when the pack ships only one image).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallpaperEntry {
  pub kind: String,
  pub id: String,
  pub name: String,
  pub path: String,
  #[serde(default)]
  pub path_dark: String,
}

/// Full wallpaper state behind `wallpaper_get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallpaperState {
  pub current: Option<WallpaperEntry>,
  pub fill: String,
  pub customs: Vec<WallpaperEntry>,
  pub premade: Vec<WallpaperEntry>,
}

/// Premade packs directory (`TONTOO_WALLPAPERS_DIR` override).
pub fn premade_dir() -> PathBuf {
  if let Ok(dir) = std::env::var("TONTOO_WALLPAPERS_DIR") {
    if !dir.is_empty() {
      return PathBuf::from(dir);
    }
  }
  PathBuf::from(DEFAULT_PREMADE_DIR)
}

/// User customs directory (`SETTINGS_WALLPAPER_DIR` override).
pub fn custom_dir() -> PathBuf {
  if let Ok(dir) = std::env::var("SETTINGS_WALLPAPER_DIR") {
    if !dir.is_empty() {
      return PathBuf::from(dir);
    }
  }
  let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
  PathBuf::from(home)
    .join("Library/Preferences")
    .join(CUSTOM_DIR_NAME)
}

/// Display name from a pack manifest (`name: "..."` line), if any.
fn fish_name(manifest: &Path) -> Option<String> {
  let content = std::fs::read_to_string(manifest).ok()?;
  for line in content.lines() {
    if let Some(rest) = line.trim().strip_prefix("name:") {
      let name = rest.trim().trim_matches('"').trim();
      if !name.is_empty() {
        return Some(name.to_string());
      }
    }
  }
  None
}

/// Manifest image file for a variant key (`light:` / `dark:` line), if any.
fn fish_image_value(manifest: &Path, key: &str) -> Option<String> {
  let content = std::fs::read_to_string(manifest).ok()?;
  let prefix = format!("{}:", key);
  for line in content.lines() {
    if let Some(rest) = line.trim().strip_prefix(prefix.as_str()) {
      let file = rest.trim().trim_matches('"').trim();
      if !file.is_empty() {
        return Some(file.to_string());
      }
    }
  }
  None
}

fn is_image_file(path: &Path) -> bool {
  matches!(
    path
      .extension()
      .and_then(|e| e.to_str())
      .map(|e| e.to_ascii_lowercase())
      .as_deref(),
    Some("png") | Some("jpg") | Some("jpeg") | Some("webp") | Some("gif") | Some("bmp")
  )
}

/// Preview image for a premade pack variant: the manifest file for the
/// variant when it exists, else the `light` file, else the first image
/// file in the directory (sorted).
pub fn resolve_pack_file(dir: &Path, variant: &str) -> Option<PathBuf> {
  let manifest = dir.join("wallpaper.fish");
  if variant == "dark" {
    if let Some(file) = fish_image_value(&manifest, "dark") {
      let candidate = dir.join(&file);
      if candidate.is_file() {
        return Some(candidate);
      }
    }
  }
  if let Some(file) = fish_image_value(&manifest, "light") {
    let candidate = dir.join(&file);
    if candidate.is_file() {
      return Some(candidate);
    }
  }
  let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
    .ok()?
    .flatten()
    .map(|e| e.path())
    .filter(|p| p.is_file() && is_image_file(p))
    .collect();
  files.sort();
  files.into_iter().next()
}

/// Preview image for a premade pack: the manifest `light` image when it
/// exists, else the first image file in the directory (sorted).
fn resolve_pack_image(dir: &Path) -> Option<PathBuf> {
  resolve_pack_file(dir, "light")
}

/// Premade packs from a directory: `(id, name, image path or "")` in
/// macOS release order (newest first). Packs without any image are listed
/// with an empty path.
pub fn scan_premade_in(dir: &Path) -> Vec<WallpaperEntry> {
  let mut packs = Vec::new();
  let entries = match std::fs::read_dir(dir) {
    Ok(entries) => entries,
    Err(_) => return packs,
  };
  for entry in entries.flatten() {
    let path = entry.path();
    if !path.is_dir() {
      continue;
    }
    let id = match path.file_name().and_then(|n| n.to_str()) {
      Some(id) => id.to_string(),
      None => continue,
    };
    let manifest = path.join("wallpaper.fish");
    let name = fish_name(&manifest).unwrap_or_else(|| id.clone());
    let image = resolve_pack_image(&path)
      .and_then(|p| p.to_str().map(str::to_string))
      .unwrap_or_default();
    let dark = resolve_pack_file(&path, "dark")
      .and_then(|p| p.to_str().map(str::to_string))
      .unwrap_or_else(|| image.clone());
    packs.push(WallpaperEntry {
      kind: KIND_PREMADE.to_string(),
      id,
      name,
      path: image,
      path_dark: dark,
    });
  }
  packs.sort_by(|a, b| {
    premade_rank(&a.id)
      .cmp(&premade_rank(&b.id))
      .then_with(|| a.name.cmp(&b.name))
  });
  packs
}

/// Premade packs from the system wallpapers directory.
pub fn scan_premade() -> Vec<WallpaperEntry> {
  scan_premade_in(&premade_dir())
}

/// `(id, path)` of every stored custom PNG, sorted by id.
pub fn scan_custom_files(dir: &Path) -> Vec<(String, PathBuf)> {
  let mut files = Vec::new();
  let entries = match std::fs::read_dir(dir) {
    Ok(entries) => entries,
    Err(_) => return files,
  };
  for entry in entries.flatten() {
    let path = entry.path();
    if !path.is_file() {
      continue;
    }
    let is_png = path
      .extension()
      .and_then(|e| e.to_str())
      .map(|e| e.eq_ignore_ascii_case("png"))
      .unwrap_or(false);
    if !is_png {
      continue;
    }
    if let Some(stem) = path.file_stem().and_then(|n| n.to_str()) {
      files.push((stem.to_string(), path));
    }
  }
  files.sort_by(|a, b| a.0.cmp(&b.0));
  files
}

/// Custom display names mirrored in `storage.fico` (`customN: id/name`).
/// Unreadable or missing files yield an empty map; callers fall back to
/// the file stem.
pub fn read_storage_names(fico: &Path) -> HashMap<String, String> {
  let mut names = HashMap::new();
  let doc = match sdk::FishFile::FishDocument::from_file(fico) {
    Ok(doc) => doc,
    Err(_) => return names,
  };
  let json = doc.to_json_value();
  if let Some(sections) = json.as_object() {
    for (_, section) in sections {
      let id = section.get("id").and_then(|v| v.as_str());
      let name = section.get("name").and_then(|v| v.as_str());
      if let (Some(id), Some(name)) = (id, name) {
        names.insert(id.to_string(), name.to_string());
      }
    }
  }
  names
}

/// Rewrite `storage.fico` from the files on disk plus known names.
pub fn write_storage_fico(
  fico: &Path,
  files: &[(String, PathBuf)],
  names: &HashMap<String, String>,
) -> std::io::Result<()> {
  use sdk::FishFile::{FishDocument, FishValue};

  if let Some(parent) = fico.parent() {
    std::fs::create_dir_all(parent)?;
  }
  let mut doc = FishDocument::new();
  for (index, (id, path)) in files.iter().enumerate() {
    let section = format!("custom{}", index);
    let name = names
      .get(id)
      .cloned()
      .unwrap_or_else(|| id.clone());
    doc.set(&format!("{}.id", section), FishValue::from(id.as_str()));
    doc.set(&format!("{}.name", section), FishValue::from(name.as_str()));
    let filename = path
      .file_name()
      .and_then(|n| n.to_str())
      .unwrap_or(id.as_str());
    doc.set(
      &format!("{}.filename", section),
      FishValue::from(filename),
    );
  }
  doc
    .write_to_file(fico)
    .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Refresh the `storage.fico` mirror from the files on disk, keeping the
/// previously stored display names.
pub fn refresh_storage(dir: &Path) -> std::io::Result<()> {
  let files = scan_custom_files(dir);
  let names = read_storage_names(&dir.join(STORAGE_FICO));
  write_storage_fico(&dir.join(STORAGE_FICO), &files, &names)
}

/// User customs: files on disk with mirrored display names, sorted by name.
pub fn scan_custom_in(dir: &Path) -> Vec<WallpaperEntry> {
  let names = read_storage_names(&dir.join(STORAGE_FICO));
  let mut entries: Vec<WallpaperEntry> = scan_custom_files(dir)
    .into_iter()
    .map(|(id, path)| {
      let name = names.get(&id).cloned().unwrap_or_else(|| id.clone());
      let path_str = path.to_str().unwrap_or_default().to_string();
      WallpaperEntry {
        kind: KIND_CUSTOM.to_string(),
        name,
        path: path_str.clone(),
        path_dark: path_str,
        id,
      }
    })
    .collect();
  entries.sort_by(|a, b| a.name.cmp(&b.name));
  entries
}

/// User customs from the preferences directory.
pub fn scan_custom() -> Vec<WallpaperEntry> {
  scan_custom_in(&custom_dir())
}

/// Filename stem sanitized for storage (`a-z 0-9 - _`, fallback `custom`).
pub fn sanitize_stem(raw: &str) -> String {
  let mut clean: String = raw
    .to_lowercase()
    .chars()
    .map(|c| {
      if c.is_ascii_alphanumeric() {
        c
      } else if c == '-' || c == '_' {
        c
      } else {
        '-'
      }
    })
    .collect();
  while clean.contains("--") {
    clean = clean.replace("--", "-");
  }
  let clean = clean.trim_matches('-').to_string();
  if clean.is_empty() {
    "custom".to_string()
  } else {
    clean
  }
}

/// Unused `stem.png` path inside `dir` (`stem-2.png`, ... on collision).
pub fn unique_png_path(dir: &Path, stem: &str) -> PathBuf {
  let candidate = dir.join(format!("{}.png", stem));
  if !candidate.exists() {
    return candidate;
  }
  for n in 2.. {
    let candidate = dir.join(format!("{}-{}.png", stem, n));
    if !candidate.exists() {
      return candidate;
    }
  }
  unreachable!("unique png path search never ends")
}

/// Decode any supported image and store it as PNG.
pub fn convert_to_png(source: &Path, dest: &Path) -> Result<(), String> {
  if !source.is_file() {
    return Err(format!("wallpaper add failed: file not found {:?}", source));
  }
  let image = image::open(source)
    .map_err(|e| format!("wallpaper add failed: unsupported image {:?}: {}", source, e))?;
  image
    .save_with_format(dest, image::ImageFormat::Png)
    .map_err(|e| format!("wallpaper add failed: cannot store PNG {:?}: {}", dest, e))?;
  Ok(())
}

fn container() -> Result<coredata::PersistentContainer, String> {
  coredata::PersistentContainer::new_with_bundle(
    crate::wifi::DAEMON_BUNDLE_ID.to_string(),
    coredata::StoreType::Fico,
  )
  .map_err(|e| e.to_string())
}

/// Delete the custom registry entry. Returns true when one existed.
fn delete_custom_entry(id: &str) -> Result<bool, String> {
  let mut store = container()?;
  let removed = {
    let mut ctx = store.view_context();
    let objects = ctx
      .fetch_all(CUSTOM_WALLPAPER_ENTITY)
      .map_err(|e| e.to_string())?;
    let ids: Vec<String> = objects
      .iter()
      .filter(|o| o.get_str("id") == Some(id))
      .map(|o| o.object_id.clone())
      .collect();
    for object_id in &ids {
      ctx.delete(object_id).map_err(|e| e.to_string())?;
    }
    if !ids.is_empty() {
      ctx.save().map_err(|e| e.to_string())?;
    }
    !ids.is_empty()
  };
  store.save().map_err(|e| e.to_string())?;
  Ok(removed)
}

/// Delete a user custom wallpaper by id. Premade packs are never
/// deletable (only the user wallpaper folder is touched). When the
/// deleted wallpaper is the current selection, first switch to Tahoe
/// Lake (auto) on the desktop, then remove the file, the CoreData entry
/// and refresh `storage.fico`. Returns whether a pre-switch happened.
/// Aborts with an error (nothing removed) when the compositor is
/// unreachable while a switch is required.
pub fn delete(store: &Arc<Mutex<SettingsStore>>, id: &str) -> Result<bool, String> {
  if id.is_empty() {
    return Err("wallpaper delete failed: id must not be empty".to_string());
  }
  let dir = custom_dir();
  let customs = scan_custom_in(&dir);
  let entry = customs
    .iter()
    .find(|e| e.id == id)
    .cloned()
    .ok_or_else(|| format!("wallpaper delete failed: unknown custom wallpaper {:?}", id))?;
  let is_current = {
    let guard = store
      .lock()
      .map_err(|_| "wallpaper delete failed: store is locked".to_string())?;
    current_in(&guard, &scan_premade(), &customs)
      .map(|current| current.kind == KIND_CUSTOM && current.id == id)
      .unwrap_or(false)
  };
  let mut switched = false;
  if is_current {
    apply(store, KIND_PREMADE, DEFAULT_PACK, "auto")?;
    switched = true;
  }
  let file = PathBuf::from(&entry.path);
  std::fs::remove_file(&file)
    .map_err(|e| format!("wallpaper delete failed: cannot remove {:?}: {}", file, e))?;
  let _ = delete_custom_entry(id);
  refresh_storage(&dir)
    .map_err(|e| format!("wallpaper delete failed: storage mirror failed: {}", e))?;
  Ok(switched)
}

/// Register the custom wallpaper in CoreData (write-only registry, the
/// listing reads the files plus `storage.fico`).
fn register_custom(id: &str, name: &str, filename: &str) -> Result<(), String> {
  let mut store = container()?;
  {
    let mut ctx = store.view_context();
    let objects = ctx
      .fetch_all(CUSTOM_WALLPAPER_ENTITY)
      .map_err(|e| e.to_string())?;
    let duplicate = objects.iter().any(|o| o.get_str("id") == Some(id));
    if !duplicate {
      let mut obj = ctx.create(CUSTOM_WALLPAPER_ENTITY);
      obj.set("id", id.to_string());
      obj.set("name", name.to_string());
      obj.set("filename", filename.to_string());
      ctx.save_object(obj).map_err(|e| e.to_string())?;
    }
    ctx.save().map_err(|e| e.to_string())?;
  }
  store.save().map_err(|e| e.to_string())
}

/// Add an image file to the user customs: decode, store as PNG, register
/// in CoreData and refresh `storage.fico`. Returns the new entry.
/// `name` overrides the display name, else the file stem is used.
pub fn add(source: &Path, name: Option<&str>) -> Result<WallpaperEntry, String> {
  let dir = custom_dir();
  std::fs::create_dir_all(&dir)
    .map_err(|e| format!("wallpaper add failed: cannot create {:?}: {}", dir, e))?;
  let fallback = source
    .file_stem()
    .and_then(|n| n.to_str())
    .unwrap_or("custom");
  let display = name
    .map(str::trim)
    .filter(|n| !n.is_empty())
    .unwrap_or(fallback)
    .to_string();
  let stem = sanitize_stem(&display);
  let dest = unique_png_path(&dir, &stem);
  let id = dest
    .file_stem()
    .and_then(|n| n.to_str())
    .unwrap_or(&stem)
    .to_string();
  convert_to_png(source, &dest)?;
  let filename = dest
    .file_name()
    .and_then(|n| n.to_str())
    .unwrap_or(&id)
    .to_string();
  if let Err(e) = register_custom(&id, &display, &filename) {
    let _ = std::fs::remove_file(&dest);
    return Err(format!("wallpaper add failed: registry failed: {}", e));
  }
  let mut names = read_storage_names(&dir.join(STORAGE_FICO));
  names.insert(id.clone(), display.clone());
  if let Err(e) = write_storage_fico(
    &dir.join(STORAGE_FICO),
    &scan_custom_files(&dir),
    &names,
  ) {
    return Err(format!(
      "wallpaper add failed: storage mirror failed: {}",
      e
    ));
  }
  Ok(WallpaperEntry {
    kind: KIND_CUSTOM.to_string(),
    id,
    name: display,
    path: dest.to_str().unwrap_or_default().to_string(),
    path_dark: dest.to_str().unwrap_or_default().to_string(),
  })
}

/// Resolve the configured current wallpaper against fresh scans.
/// A fresh config (both keys absent) selects `DEFAULT_PACK` when the pack
/// exists, so new installs boot Tahoe Lake. Stored but unknown ids yield
/// `None` (shows "No wallpaper set").
pub fn current_in(
  store: &SettingsStore,
  premade: &[WallpaperEntry],
  customs: &[WallpaperEntry],
) -> Option<WallpaperEntry> {
  let kind = store
    .get(DOMAIN, KEY_CURRENT_KIND)
    .and_then(|v| v.as_str())
    .unwrap_or("");
  let id = store
    .get(DOMAIN, KEY_CURRENT_ID)
    .and_then(|v| v.as_str())
    .unwrap_or("");
  if kind.is_empty() && id.is_empty() {
    return premade
      .iter()
      .find(|e| e.id == DEFAULT_PACK)
      .cloned();
  }
  if id.is_empty() {
    return None;
  }
  let list = match kind {
    KIND_PREMADE => premade,
    KIND_CUSTOM => customs,
    _ => return None,
  };
  list.iter().find(|e| e.id == id).cloned()
}

/// Configured fill mode, validated, defaulting to `fill`.
pub fn fill_in(store: &SettingsStore) -> String {
  let fill = store
    .get(DOMAIN, KEY_FILL)
    .and_then(|v| v.as_str())
    .unwrap_or(DEFAULT_FILL);
  if FILL_MODES.contains(&fill) {
    fill.to_string()
  } else {
    DEFAULT_FILL.to_string()
  }
}

/// Full wallpaper state: fresh scans plus stored current and fill.
pub fn state(store: &SettingsStore) -> WallpaperState {
  let premade = scan_premade();
  let customs = scan_custom();
  let current = current_in(store, &premade, &customs);
  WallpaperState {
    current,
    fill: fill_in(store),
    customs,
    premade,
  }
}

/// Persist the current wallpaper selection (desktop untouched).
/// The kind/id must resolve against fresh scans.
pub fn set_current(
  store: &Arc<Mutex<SettingsStore>>,
  kind: &str,
  id: &str,
) -> Result<Option<WallpaperEntry>, String> {
  if kind != KIND_PREMADE && kind != KIND_CUSTOM {
    return Err(format!("wallpaper set failed: unknown kind {:?}", kind));
  }
  if id.is_empty() {
    return Err("wallpaper set failed: id must not be empty".to_string());
  }
  let mut guard = store
    .lock()
    .map_err(|_| "wallpaper set failed: store is locked".to_string())?;
  let premade = scan_premade();
  let customs = scan_custom();
  let list = if kind == KIND_PREMADE {
    &premade
  } else {
    &customs
  };
  let entry = list
    .iter()
    .find(|e| e.id == id)
    .cloned()
    .ok_or_else(|| format!("wallpaper set failed: unknown wallpaper {:?}", id))?;
  guard.set(DOMAIN, KEY_CURRENT_KIND, serde_json::json!(kind));
  guard.set(DOMAIN, KEY_CURRENT_ID, serde_json::json!(id));
  guard
    .save()
    .map_err(|e| format!("wallpaper set failed: store save failed: {}", e))?;
  Ok(Some(entry))
}

/// Persist the fill mode and apply it live: forwards the current
/// wallpaper file with the new mode to the compositor first, then
/// persists. Nothing is persisted when the compositor is unreachable
/// (same strictness as `apply`). With no wallpaper configured the mode
/// only persists.
pub fn set_fill(store: &Arc<Mutex<SettingsStore>>, fill: &str) -> Result<String, String> {
  if !FILL_MODES.contains(&fill) {
    return Err(format!("wallpaper set failed: unknown fill mode {:?}", fill));
  }
  let current_file: Option<PathBuf> = {
    let guard = store
      .lock()
      .map_err(|_| "wallpaper set failed: store is locked".to_string())?;
    current_in(&guard, &scan_premade(), &scan_custom())
      .map(|entry| PathBuf::from(entry.path))
  };
  if let Some(file) = current_file {
    if !file.is_file() {
      return Err(format!("wallpaper set failed: file not found {:?}", file));
    }
    send_to_compositor(&file, fill)?;
  }
  let mut guard = store
    .lock()
    .map_err(|_| "wallpaper set failed: store is locked".to_string())?;
  guard.set(DOMAIN, KEY_FILL, serde_json::json!(fill));
  guard
    .save()
    .map_err(|e| format!("wallpaper set failed: store save failed: {}", e))?;
  Ok(fill.to_string())
}

/// Compositor settings socket (`COMPOSITOR_SOCKET` override).
pub fn compositor_socket() -> PathBuf {
  if let Ok(sock) = std::env::var("COMPOSITOR_SOCKET") {
    if !sock.is_empty() {
      return PathBuf::from(sock);
    }
  }
  PathBuf::from(DEFAULT_COMPOSITOR_SOCKET)
}

/// Active theme backing the `auto` variant (`customize` domain).
fn active_theme(store: &SettingsStore) -> String {
  let theme = store
    .get("customize", "theme")
    .and_then(|v| v.as_str())
    .unwrap_or("dark");
  if theme == "light" {
    "light".to_string()
  } else {
    "dark".to_string()
  }
}

/// Forward a resolved image file plus fill mode to the compositor for
/// the desktop crossfade. The compositor validates and loads the file
/// itself.
pub(crate) fn send_to_compositor(file: &Path, fill: &str) -> Result<(), String> {
  use std::io::{BufRead, BufReader, Write};
  use std::time::Duration;

  let sock = compositor_socket();
  let mut stream = std::os::unix::net::UnixStream::connect(&sock).map_err(|e| {
    format!(
      "wallpaper apply failed: compositor unreachable at {}: {}",
      sock.display(),
      e
    )
  })?;
  let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
  let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
  let mut line = serde_json::json!({"op": "set_wallpaper", "path": file.to_string_lossy(), "fill": fill}).to_string();
  line.push('\n');
  stream
    .write_all(line.as_bytes())
    .map_err(|e| format!("wallpaper apply failed: compositor write failed: {}", e))?;
  stream
    .flush()
    .map_err(|e| format!("wallpaper apply failed: compositor write failed: {}", e))?;
  let mut reader = BufReader::new(&stream);
  let mut reply = String::new();
  reader
    .read_line(&mut reply)
    .map_err(|e| format!("wallpaper apply failed: compositor read failed: {}", e))?;
  let frame: serde_json::Value = serde_json::from_str(&reply)
    .map_err(|e| format!("wallpaper apply failed: compositor reply invalid: {}", e))?;
  if frame.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
    Ok(())
  } else {
    Err(format!(
      "wallpaper apply failed: {}",
      frame.get("error").and_then(|v| v.as_str()).unwrap_or("compositor error")
    ))
  }
}

/// Push the configured current wallpaper to the compositor (startup
/// sync, best effort). Resolves like `current_in`, honoring the
/// `DEFAULT_PACK` fallback, and forwards the entry file with the
/// configured fill mode. Returns the pushed entry, or `None` when
/// nothing is configured. Errors when the compositor is unreachable or
/// the file is missing.
pub fn push_current_to_compositor(store: &SettingsStore) -> Result<Option<WallpaperEntry>, String> {
  let premade = scan_premade();
  let customs = scan_custom();
  let Some(entry) = current_in(store, &premade, &customs) else {
    return Ok(None);
  };
  let file = PathBuf::from(&entry.path);
  if !file.is_file() {
    return Err(format!(
      "wallpaper push failed: file not found {:?}",
      file
    ));
  }
  send_to_compositor(&file, &fill_in(store))?;
  Ok(Some(entry))
}

/// Apply a wallpaper to the desktop: resolve the variant file, start the
/// compositor crossfade, then persist the selection. Nothing is persisted
/// when the compositor is unreachable. `variant` is `light`, `dark` or
/// `auto` (follows the `customize` theme). Returns the applied entry with
/// `path` set to the resolved file.
pub fn apply(
  store: &Arc<Mutex<SettingsStore>>,
  kind: &str,
  id: &str,
  variant: &str,
) -> Result<WallpaperEntry, String> {
  if !VARIANTS.contains(&variant) {
    return Err(format!("wallpaper apply failed: unknown variant {:?}", variant));
  }
  if kind != KIND_PREMADE && kind != KIND_CUSTOM {
    return Err(format!("wallpaper apply failed: unknown kind {:?}", kind));
  }
  if id.is_empty() {
    return Err("wallpaper apply failed: id must not be empty".to_string());
  }
  let premade = scan_premade();
  let customs = scan_custom();
  let list = if kind == KIND_PREMADE {
    &premade
  } else {
    &customs
  };
  let entry = list
    .iter()
    .find(|e| e.id == id)
    .cloned()
    .ok_or_else(|| format!("wallpaper apply failed: unknown wallpaper {:?}", id))?;
  let file = if kind == KIND_CUSTOM {
    PathBuf::from(&entry.path)
  } else {
    let theme = if variant == "auto" {
      let guard = store
        .lock()
        .map_err(|_| "wallpaper apply failed: store is locked".to_string())?;
      active_theme(&guard)
    } else {
      variant.to_string()
    };
    resolve_pack_file(&premade_dir().join(id), &theme)
      .ok_or_else(|| format!("wallpaper apply failed: no image file for {:?}", id))?
  };
  if !file.is_file() {
    return Err(format!(
      "wallpaper apply failed: file not found {:?}",
      file
    ));
  }
  let fill = {
    let guard = store
      .lock()
      .map_err(|_| "wallpaper apply failed: store is locked".to_string())?;
    fill_in(&guard)
  };
  send_to_compositor(&file, &fill)?;
  set_current(store, kind, id)?;
  let mut applied = entry;
  applied.path = file.to_str().unwrap_or_default().to_string();
  Ok(applied)
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;
  use std::path::PathBuf;

  fn temp_case(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tontoo-wallpaper-{}", name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
  }

  fn write_pack(dir: &Path, id: &str, manifest: Option<&str>, images: &[&str]) {
    let pack = dir.join(id);
    std::fs::create_dir_all(&pack).unwrap();
    if let Some(body) = manifest {
      std::fs::write(pack.join("wallpaper.fish"), body).unwrap();
    }
    for image in images {
      std::fs::write(pack.join(image), b"fake").unwrap();
    }
  }

  fn memory_store(path: &Path) -> Arc<Mutex<SettingsStore>> {
    Arc::new(Mutex::new(SettingsStore::new(path.to_path_buf())))
  }

  #[test]
  fn premade_scan_prefers_manifest_image_and_name() {    let dir = temp_case("premade");
    write_pack(
      &dir,
      "THAOELAKE",
      Some("name: \"Tahoe Lake\"\nimages:\n  light: \"IMAGE.png\"\n"),
      &["IMAGE.png", "OTHER.jpg"],
    );
    write_pack(&dir, "VENTURA", None, &["wall.jpg"]);
    write_pack(&dir, "EMPTY", None, &[]);
    std::fs::write(dir.join("stray.txt"), "not a pack").unwrap();

    let packs = scan_premade_in(&dir);
    assert_eq!(packs.len(), 3);
    let tahoe = packs.iter().find(|p| p.id == "THAOELAKE").unwrap();
    assert_eq!(tahoe.name, "Tahoe Lake");
    assert!(tahoe.path.ends_with("IMAGE.png"));
    let ventura = packs.iter().find(|p| p.id == "VENTURA").unwrap();
    assert_eq!(ventura.name, "VENTURA");
    assert!(ventura.path.ends_with("wall.jpg"));
    let empty = packs.iter().find(|p| p.id == "EMPTY").unwrap();
    assert_eq!(empty.path, "");
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn premade_scan_orders_by_macos_release() {
    let dir = temp_case("order");
    for id in ["MOJAVE", "GOLDENGATE", "THAOELAKE", "SONOMA", "ZZZNEW", "FLOW"] {
      write_pack(&dir, id, None, &["a.png"]);
    }
    let packs = scan_premade_in(&dir);
    let ids: Vec<&str> = packs.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
      ids,
      vec!["GOLDENGATE", "THAOELAKE", "SONOMA", "MOJAVE", "FLOW", "ZZZNEW"]
    );
    assert_eq!(premade_rank("GOLDENGATE"), 0);
    assert_eq!(premade_rank("ZZZNEW"), PREMADE_ORDER.len());
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn missing_premade_dir_yields_no_packs() {
    let packs = scan_premade_in(Path::new("/nonexistent-wallpaper-test"));
    assert!(packs.is_empty());
  }

  #[test]
  fn sanitize_and_unique_names() {
    assert_eq!(sanitize_stem("My Photo 2024!"), "my-photo-2024");
    assert_eq!(sanitize_stem(""), "custom");
    assert_eq!(sanitize_stem("---"), "custom");
    let dir = temp_case("unique");
    assert_eq!(
      unique_png_path(&dir, "shot"),
      dir.join("shot.png")
    );
    std::fs::write(dir.join("shot.png"), b"x").unwrap();
    assert_eq!(
      unique_png_path(&dir, "shot"),
      dir.join("shot-2.png")
    );
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn storage_fico_roundtrip() {
    let dir = temp_case("fico");
    let files = vec![
      ("reef".to_string(), dir.join("reef.png")),
      ("dune".to_string(), dir.join("dune.png")),
    ];
    let mut names = HashMap::new();
    names.insert("reef".to_string(), "Tontoo Reef".to_string());
    write_storage_fico(&dir.join(STORAGE_FICO), &files, &names).unwrap();
    let back = read_storage_names(&dir.join(STORAGE_FICO));
    assert_eq!(back.get("reef").map(String::as_str), Some("Tontoo Reef"));
    assert_eq!(back.get("dune").map(String::as_str), Some("dune"));
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn missing_storage_fico_yields_no_names() {
    let names = read_storage_names(Path::new("/nonexistent-wallpaper-test/storage.fico"));
    assert!(names.is_empty());
  }

  #[test]
  fn convert_rejects_missing_and_non_images() {
    let dir = temp_case("convert");
    assert!(convert_to_png(Path::new("/nonexistent-wallpaper-test/x.png"), &dir.join("o.png")).is_err());
    let bad = dir.join("note.txt");
    std::fs::write(&bad, b"hello").unwrap();
    assert!(convert_to_png(&bad, &dir.join("o.png")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn convert_stores_png_with_magic_bytes() {
    let dir = temp_case("convert-png");
    let img = image::RgbImage::new(4, 3);
    let source = dir.join("source.jpg");
    image::DynamicImage::ImageRgb8(img)
      .save_with_format(&source, image::ImageFormat::Jpeg)
      .unwrap();
    let dest = dir.join("custom.png");
    convert_to_png(&source, &dest).unwrap();
    let bytes = std::fs::read(&dest).unwrap();
    assert_eq!(&bytes[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn set_fill_validates_and_persists() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    // No THAOELAKE pack here: nothing configured, persist-only path.
    let premade = temp_case("fill-premade");
    write_pack(&premade, "FLOW", Some("name: \"Flow\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("fill-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let dir = temp_case("fill");
    let store = memory_store(&dir.join("settings.json"));
    assert_eq!(fill_in(&store.lock().unwrap()), DEFAULT_FILL);
    assert_eq!(set_fill(&store, "tile").unwrap(), "tile");
    assert_eq!(fill_in(&store.lock().unwrap()), "tile");
    assert!(set_fill(&store, "melt").is_err());
    assert_eq!(fill_in(&store.lock().unwrap()), "tile");
    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn set_fill_forwards_live_and_fails_cleanly() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let premade = temp_case("fill-live-premade");
    write_pack(&premade, "FLOW", Some("name: \"Flow\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("fill-live-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let dir = temp_case("fill-live-store");
    let store = memory_store(&dir.join("settings.json"));
    set_current(&store, "premade", "FLOW").unwrap();

    // Compositor down: error, fill not persisted.
    std::env::set_var(
      "COMPOSITOR_SOCKET",
      "/nonexistent-wallpaper-test/compositor.sock",
    );
    assert!(set_fill(&store, "tile").is_err());
    assert_eq!(fill_in(&store.lock().unwrap()), DEFAULT_FILL);

    // Compositor up: applied live and persisted.
    let sock = mock_compositor(serde_json::json!({"ok": true, "result": {"fading": true}}));
    std::env::set_var("COMPOSITOR_SOCKET", &sock);
    assert_eq!(set_fill(&store, "tile").unwrap(), "tile");
    assert_eq!(fill_in(&store.lock().unwrap()), "tile");

    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    std::env::remove_var("COMPOSITOR_SOCKET");
    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn apply_frame_carries_fill_mode() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex as StdMutex};

    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let premade = temp_case("frame-premade");
    write_pack(&premade, "FLOW", Some("name: \"Flow\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("frame-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let dir = temp_case("frame-store");
    let store = memory_store(&dir.join("settings.json"));
    {
      let mut guard = store.lock().unwrap();
      guard.set(DOMAIN, KEY_FILL, serde_json::json!("center"));
      guard.save().unwrap();
    }

    // Capture listener: records the request line, replies ok.
    let path = std::env::temp_dir().join(format!(
      "tontoo-compositor-capture-{}.sock",
      std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let captured: Arc<StdMutex<String>> = Arc::new(StdMutex::new(String::new()));
    let captured_thread = captured.clone();
    std::thread::spawn(move || {
      if let Ok((mut stream, _)) = listener.accept() {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        *captured_thread.lock().unwrap() = line;
        let _ = stream.write_all(b"{\"ok\": true, \"result\": {}}\n");
      }
    });
    std::env::set_var("COMPOSITOR_SOCKET", &path);
    apply(&store, "premade", "FLOW", "light").unwrap();
    let line = captured.lock().unwrap().clone();
    let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["op"], "set_wallpaper");
    assert_eq!(frame["fill"], "center");
    assert!(frame["path"].as_str().unwrap().ends_with("a.png"));

    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    std::env::remove_var("COMPOSITOR_SOCKET");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn set_current_validates_against_scans() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let premade = temp_case("current-premade");
    write_pack(&premade, "FLOW", Some("name: \"Flow\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("current-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let dir = temp_case("current-store");
    let store = memory_store(&dir.join("settings.json"));

    {
      let guard = store.lock().unwrap();
      assert!(current_in(&guard, &scan_premade(), &scan_custom()).is_none());
    }
    assert!(set_current(&store, "orb", "FLOW").is_err());
    assert!(set_current(&store, "premade", "MISSING").is_err());
    let entry = set_current(&store, "premade", "FLOW").unwrap().unwrap();
    assert_eq!(entry.name, "Flow");
    {
      let guard = store.lock().unwrap();
      assert_eq!(
        current_in(&guard, &scan_premade(), &scan_custom()).map(|e| e.id),
        Some("FLOW".to_string())
      );
    }
    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn state_lists_scans_with_defaults() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let premade = temp_case("state-premade");
    write_pack(&premade, "FLOW", Some("name: \"Flow\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("state-custom");
    std::fs::write(customs.join("mine.png"), b"x").unwrap();
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let dir = temp_case("state-store");
    let store = memory_store(&dir.join("settings.json"));

    let guard = store.lock().unwrap();
    let state = state(&guard);
    assert!(state.current.is_none());
    assert_eq!(state.fill, DEFAULT_FILL);
    assert_eq!(state.premade.len(), 1);
    assert_eq!(state.customs.len(), 1);
    assert_eq!(state.customs[0].id, "mine");
    assert_eq!(state.customs[0].name, "mine");
    drop(guard);
    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn fresh_config_defaults_to_tahoe_lake() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let premade = temp_case("default-premade");
    write_pack(&premade, "THAOELAKE", Some("name: \"Tahoe Lake\"\n"), &["a.png"]);
    write_pack(&premade, "FLOW", Some("name: \"Flow\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("default-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let dir = temp_case("default-store");
    let store = memory_store(&dir.join("settings.json"));

    let guard = store.lock().unwrap();
    let current = current_in(&guard, &scan_premade(), &scan_custom());
    assert_eq!(current.map(|e| e.id), Some("THAOELAKE".to_string()));
    drop(guard);
    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn push_forwards_default_and_skips_gracefully() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let premade = temp_case("push-premade");
    write_pack(&premade, "THAOELAKE", Some("name: \"Tahoe Lake\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("push-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let dir = temp_case("push-store");
    let store = SettingsStore::new(dir.join("settings.json"));

    // No compositor: graceful error, nothing persisted.
    std::env::set_var(
      "COMPOSITOR_SOCKET",
      "/nonexistent-wallpaper-test/compositor.sock",
    );
    assert!(push_current_to_compositor(&store).is_err());

    // Mock compositor: default pushes Tahoe Lake.
    let sock = mock_compositor(serde_json::json!({"ok": true, "result": {"fading": true}}));
    std::env::set_var("COMPOSITOR_SOCKET", &sock);
    let pushed = push_current_to_compositor(&store).unwrap().unwrap();
    assert_eq!(pushed.id, "THAOELAKE");

    // Empty scans: nothing to push.
    std::env::set_var("TONTOO_WALLPAPERS_DIR", "/nonexistent-wallpaper-test");
    assert!(push_current_to_compositor(&store).unwrap().is_none());

    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    std::env::remove_var("COMPOSITOR_SOCKET");
    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn full_add_flow_with_temp_home() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    // Exercise add() end to end except the CoreData registry write:
    // the registry needs the on-device store, the file + fico flow is
    // covered here through the pure helpers.
    let dir = temp_case("add");
    let img = image::RgbImage::new(2, 2);
    let source = dir.join("photo.jpg");
    image::DynamicImage::ImageRgb8(img)
      .save_with_format(&source, image::ImageFormat::Jpeg)
      .unwrap();
    let dest = unique_png_path(&dir, &sanitize_stem("My Photo"));
    convert_to_png(&source, &dest).unwrap();
    assert!(dest.is_file());
    let mut names = HashMap::new();
    names.insert("my-photo".to_string(), "My Photo".to_string());
    write_storage_fico(&dir.join(STORAGE_FICO), &scan_custom_files(&dir), &names).unwrap();
    let entries = scan_custom_in(&dir);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "My Photo");
    let _ = std::fs::remove_dir_all(&dir);
  }

  /// Mock compositor socket: accept one connection, read the request line,
  /// reply with one frame. Returns the socket path (unique per call).
  fn mock_compositor(reply: serde_json::Value) -> PathBuf {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
      "tontoo-compositor-mock-{}-{}.sock",
      std::process::id(),
      NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
      if let Ok((mut stream, _)) = listener.accept() {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let mut out = reply.to_string();
        out.push('\n');
        let _ = stream.write_all(out.as_bytes());
      }
    });
    path
  }

  #[test]
  fn apply_validates_before_touching_anything() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let dir = temp_case("apply-validate");
    let store = memory_store(&dir.join("settings.json"));
    assert!(apply(&store, "premade", "FLOW", "sepia").is_err());
    assert!(apply(&store, "orb", "FLOW", "light").is_err());
    assert!(apply(&store, "premade", "", "light").is_err());
    assert!(apply(&store, "premade", "MISSING", "light").is_err());
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn apply_forwards_then_persists() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let premade = temp_case("apply-premade");
    write_pack(
      &premade,
      "FLOW",
      Some("name: \"Flow\"\nimages:\n  light: \"day.png\"\n  dark: \"night.png\"\n"),
      &["day.png", "night.png"],
    );
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("apply-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let sock = mock_compositor(serde_json::json!({"ok": true, "result": {"fading": true}}));
    std::env::set_var("COMPOSITOR_SOCKET", &sock);
    let dir = temp_case("apply-store");
    let store = memory_store(&dir.join("settings.json"));

    let applied = apply(&store, "premade", "FLOW", "dark").unwrap();
    assert_eq!(applied.id, "FLOW");
    assert!(applied.path.ends_with("night.png"));
    {
      let guard = store.lock().unwrap();
      assert_eq!(
        current_in(&guard, &scan_premade(), &scan_custom()).map(|e| e.id),
        Some("FLOW".to_string())
      );
    }
    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    std::env::remove_var("COMPOSITOR_SOCKET");
    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn apply_fails_cleanly_without_compositor() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let premade = temp_case("apply-nocomp");
    write_pack(&premade, "FLOW", Some("name: \"Flow\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("apply-nocomp-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    std::env::set_var(
      "COMPOSITOR_SOCKET",
      "/nonexistent-wallpaper-test/compositor.sock",
    );
    let dir = temp_case("apply-nocomp-store");
    let store = memory_store(&dir.join("settings.json"));

    let err = apply(&store, "premade", "FLOW", "light").unwrap_err();
    assert!(err.contains("unreachable"));
    {
      let guard = store.lock().unwrap();
      assert!(current_in(&guard, &scan_premade(), &scan_custom()).is_none());
    }
    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    std::env::remove_var("COMPOSITOR_SOCKET");
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  fn sandbox_home(name: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("tontoo-home-{}", name));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    home
  }

  fn write_custom_png(dir: &Path, name: &str) -> PathBuf {
    let img = image::RgbImage::new(4, 3);
    let path = dir.join(name);
    image::DynamicImage::ImageRgb8(img)
      .save_with_format(&path, image::ImageFormat::Png)
      .unwrap();
    path
  }

  #[test]
  fn delete_rejects_bad_ids() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let dir = temp_case("delete-validate");
    let store = memory_store(&dir.join("settings.json"));
    assert!(delete(&store, "").is_err());
    assert!(delete(&store, "ghost").is_err());
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn delete_non_current_removes_file_and_mirror() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let home = sandbox_home("delete-plain");
    let customs = temp_case("delete-plain-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let file = write_custom_png(&customs, "mine.png");
    let dir = temp_case("delete-plain-store");
    let store = memory_store(&dir.join("settings.json"));

    let switched = delete(&store, "mine").unwrap();
    assert!(!switched);
    assert!(!file.exists());
    assert!(scan_custom_in(&customs).is_empty());

    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    std::env::remove_var("HOME");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn delete_current_switches_to_tahoe_lake_first() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let home = sandbox_home("delete-current");
    let premade = temp_case("delete-current-premade");
    write_pack(&premade, "THAOELAKE", Some("name: \"Tahoe Lake\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("delete-current-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let file = write_custom_png(&customs, "mine.png");
    let sock = mock_compositor(serde_json::json!({"ok": true, "result": {"fading": true}}));
    std::env::set_var("COMPOSITOR_SOCKET", &sock);
    let dir = temp_case("delete-current-store");
    let store = memory_store(&dir.join("settings.json"));
    set_current(&store, "custom", "mine").unwrap();

    let switched = delete(&store, "mine").unwrap();
    assert!(switched);
    assert!(!file.exists());
    {
      let guard = store.lock().unwrap();
      assert_eq!(
        current_in(&guard, &scan_premade(), &scan_custom()).map(|e| e.id),
        Some("THAOELAKE".to_string())
      );
    }
    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    std::env::remove_var("COMPOSITOR_SOCKET");
    std::env::remove_var("HOME");
    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn delete_current_aborts_when_compositor_down() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let home = sandbox_home("delete-abort");
    let premade = temp_case("delete-abort-premade");
    write_pack(&premade, "THAOELAKE", Some("name: \"Tahoe Lake\"\n"), &["a.png"]);
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("delete-abort-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let file = write_custom_png(&customs, "mine.png");
    std::env::set_var(
      "COMPOSITOR_SOCKET",
      "/nonexistent-wallpaper-test/compositor.sock",
    );
    let dir = temp_case("delete-abort-store");
    let store = memory_store(&dir.join("settings.json"));
    set_current(&store, "custom", "mine").unwrap();

    let err = delete(&store, "mine").unwrap_err();
    assert!(err.contains("unreachable"));
    assert!(file.exists());

    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    std::env::remove_var("COMPOSITOR_SOCKET");
    std::env::remove_var("HOME");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn json_shape_matches_socket_contract() {
    let entry = WallpaperEntry {
      kind: KIND_CUSTOM.to_string(),
      id: "mine".to_string(),
      name: "Mine".to_string(),
      path: "/tmp/mine.png".to_string(),
      path_dark: "/tmp/mine.png".to_string(),
    };
    let value = serde_json::to_value(&entry).unwrap();
    assert_eq!(value, json!({"kind": "custom", "id": "mine", "name": "Mine", "path": "/tmp/mine.png", "path_dark": "/tmp/mine.png"}));
    // Older replies without path_dark still parse (default "").
    let legacy: WallpaperEntry = serde_json::from_value(
      json!({"kind": "premade", "id": "x", "name": "X", "path": "/tmp/x.png"}),
    )
    .unwrap();
    assert_eq!(legacy.path_dark, "");
  }

  #[test]
  fn variant_files_resolve_with_fallback() {
    let dir = temp_case("variants");
    write_pack(
      &dir,
      "DUO",
      Some("name: \"Duo\"\nimages:\n  light: \"day.png\"\n  dark: \"night.png\"\n"),
      &["day.png", "night.png"],
    );
    write_pack(&dir, "SOLO", Some("name: \"Solo\"\n"), &["only.png"]);
    let pack = dir.join("DUO");
    assert!(resolve_pack_file(&pack, "light").unwrap().ends_with("day.png"));
    assert!(resolve_pack_file(&pack, "dark").unwrap().ends_with("night.png"));
    let solo = dir.join("SOLO");
    assert!(resolve_pack_file(&solo, "dark").unwrap().ends_with("only.png"));
    let entries = scan_premade_in(&dir);
    let duo = entries.iter().find(|e| e.id == "DUO").unwrap();
    assert!(duo.path.ends_with("day.png"));
    assert!(duo.path_dark.ends_with("night.png"));
    let _ = std::fs::remove_dir_all(&dir);
  }
}
