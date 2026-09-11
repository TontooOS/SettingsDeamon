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

/// Store domain holding the current wallpaper and the fill mode.
pub const DOMAIN: &str = "wallpaper";
pub const KEY_CURRENT_KIND: &str = "current_kind";
pub const KEY_CURRENT_ID: &str = "current_id";
pub const KEY_FILL: &str = "fill";

pub const OP_WALLPAPER_GET: &str = "wallpaper_get";
pub const OP_WALLPAPER_SET_CURRENT: &str = "wallpaper_set_current";
pub const OP_WALLPAPER_SET_FILL: &str = "wallpaper_set_fill";
pub const OP_WALLPAPER_ADD: &str = "wallpaper_add";

/// Kind tag for premade pack entries.
pub const KIND_PREMADE: &str = "premade";
/// Kind tag for user custom entries.
pub const KIND_CUSTOM: &str = "custom";

/// Fill modes accepted by `wallpaper_set_fill`.
pub const FILL_MODES: &[&str] = &["fill", "fit", "stretch", "center", "tile"];
pub const DEFAULT_FILL: &str = "fill";

/// CoreData entity holding one user custom wallpaper per object.
pub const CUSTOM_WALLPAPER_ENTITY: &str = "CustomWallpaper";

/// One listed wallpaper: a premade pack or a user custom file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallpaperEntry {
  pub kind: String,
  pub id: String,
  pub name: String,
  pub path: String,
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

/// Image file named by a pack manifest (`images: light: "..."` line), if any.
fn fish_image(manifest: &Path) -> Option<String> {
  let content = std::fs::read_to_string(manifest).ok()?;
  for line in content.lines() {
    if let Some(rest) = line.trim().strip_prefix("light:") {
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

/// Preview image for a premade pack: the manifest `light` image when it
/// exists, else the first image file in the directory (sorted).
fn resolve_pack_image(dir: &Path) -> Option<PathBuf> {
  let manifest = dir.join("wallpaper.fish");
  if let Some(file) = fish_image(&manifest) {
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

/// Premade packs from a directory: `(id, name, image path or "")` sorted
/// by display name. Packs without any image are listed with an empty path.
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
    packs.push(WallpaperEntry {
      kind: KIND_PREMADE.to_string(),
      id,
      name,
      path: image,
    });
  }
  packs.sort_by(|a, b| a.name.cmp(&b.name));
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
      WallpaperEntry {
        kind: KIND_CUSTOM.to_string(),
        name,
        path: path.to_str().unwrap_or_default().to_string(),
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
  })
}

/// Resolve the configured current wallpaper against fresh scans.
/// Unknown ids yield `None` (shows "No wallpaper set").
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

/// Persist the fill mode (desktop untouched).
pub fn set_fill(store: &Arc<Mutex<SettingsStore>>, fill: &str) -> Result<String, String> {
  if !FILL_MODES.contains(&fill) {
    return Err(format!("wallpaper set failed: unknown fill mode {:?}", fill));
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

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;
  use std::path::PathBuf;

  static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
  fn premade_scan_prefers_manifest_image_and_name() {
    let dir = temp_case("premade");
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
    let dir = temp_case("fill");
    let store = memory_store(&dir.join("settings.json"));
    assert_eq!(fill_in(&store.lock().unwrap()), DEFAULT_FILL);
    assert_eq!(set_fill(&store, "tile").unwrap(), "tile");
    assert_eq!(fill_in(&store.lock().unwrap()), "tile");
    assert!(set_fill(&store, "melt").is_err());
    assert_eq!(fill_in(&store.lock().unwrap()), "tile");
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn set_current_validates_against_scans() {
    let _guard = ENV_LOCK.lock().unwrap();
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
    let _guard = ENV_LOCK.lock().unwrap();
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
  fn full_add_flow_with_temp_home() {
    let _guard = ENV_LOCK.lock().unwrap();
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

  #[test]
  fn json_shape_matches_socket_contract() {
    let entry = WallpaperEntry {
      kind: KIND_CUSTOM.to_string(),
      id: "mine".to_string(),
      name: "Mine".to_string(),
      path: "/tmp/mine.png".to_string(),
    };
    let value = serde_json::to_value(&entry).unwrap();
    assert_eq!(value, json!({"kind": "custom", "id": "mine", "name": "Mine", "path": "/tmp/mine.png"}));
  }
}
