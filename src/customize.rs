//! Customize backend owned by the settings daemon.
//!
//! Persists the desktop customization (wallpaper pack, accent color, theme)
//! in the `customize` store domain and serves it over the socket:
//! `customize_get` is a public read op, `customize_set` is a private write
//! op with no public client library, reserved for the Settings app
//! (`com.tontoo.systemsettings`).
//!
//! Stored values are validated on read and on write: unknown accents or
//! themes fall back to (read) or are rejected with (write) an error, so a
//! corrupt store file can never produce an invalid reply.

use std::sync::{Arc, Mutex};

use crate::json::JsonValue;
use crate::store::SettingsStore;

/// Store domain holding the customization keys.
pub const DOMAIN: &str = "customize";
/// Wallpaper pack id (e.g. `"THAOELAKE"`).
pub const KEY_WALLPAPER: &str = "wallpaper";
/// Accent color name (one of `ACCENTS`).
pub const KEY_ACCENT: &str = "accent";
/// Color theme (one of `THEMES`).
pub const KEY_THEME: &str = "theme";
/// Liquid glass amount (one of `GLASS_AMOUNTS`).
pub const KEY_GLASS: &str = "glass";
/// Revision counter, bumped on every successful `customize_set` so clients
/// can poll cheaply for changes.
pub const KEY_REVISION: &str = "revision";

pub const OP_CUSTOMIZE_GET: &str = "customize_get";
pub const OP_CUSTOMIZE_SET: &str = "customize_set";

pub const DEFAULT_WALLPAPER: &str = "THAOELAKE";
pub const DEFAULT_ACCENT: &str = "multicolor";
pub const DEFAULT_THEME: &str = "dark";
pub const DEFAULT_GLASS: &str = "glass";

/// Accent colors accepted by `customize_set`. Mirrors the Settings app
/// palette plus the `multicolor` default element (renders as blue).
pub const ACCENTS: &[&str] = &[
  "multicolor",
  "blue",
  "red",
  "orange",
  "yellow",
  "green",
  "teal",
  "cyan",
  "indigo",
  "purple",
  "purple2",
  "pink",
  "gray",
];
/// Themes accepted by `customize_set`.
pub const THEMES: &[&str] = &["dark", "light"];
/// Liquid glass amounts accepted by `customize_set`: the "LiquidGlass
/// Slider" with much glass, balanced glass and less glass.
pub const GLASS_AMOUNTS: &[&str] = &["much", "glass", "less"];

/// Color theme with typed dark/light handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
  Dark,
  Light,
}

impl ThemeMode {
  pub fn as_str(&self) -> &'static str {
    match self {
      Self::Dark => "dark",
      Self::Light => "light",
    }
  }

  pub fn from_str(raw: &str) -> Option<Self> {
    match raw {
      "dark" => Some(Self::Dark),
      "light" => Some(Self::Light),
      _ => None,
    }
  }
}

/// Accent color with typed handling. `Multicolor` is the default element
/// and renders as blue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccentColor {
  Multicolor,
  Blue,
  Red,
  Orange,
  Yellow,
  Green,
  Teal,
  Cyan,
  Indigo,
  Purple,
  Purple2,
  Pink,
  Gray,
}

impl AccentColor {
  pub fn as_str(&self) -> &'static str {
    match self {
      Self::Multicolor => "multicolor",
      Self::Blue => "blue",
      Self::Red => "red",
      Self::Orange => "orange",
      Self::Yellow => "yellow",
      Self::Green => "green",
      Self::Teal => "teal",
      Self::Cyan => "cyan",
      Self::Indigo => "indigo",
      Self::Purple => "purple",
      Self::Purple2 => "purple2",
      Self::Pink => "pink",
      Self::Gray => "gray",
    }
  }

  pub fn from_str(raw: &str) -> Option<Self> {
    match raw {
      "multicolor" => Some(Self::Multicolor),
      "blue" => Some(Self::Blue),
      "red" => Some(Self::Red),
      "orange" => Some(Self::Orange),
      "yellow" => Some(Self::Yellow),
      "green" => Some(Self::Green),
      "teal" => Some(Self::Teal),
      "cyan" => Some(Self::Cyan),
      "indigo" => Some(Self::Indigo),
      "purple" => Some(Self::Purple),
      "purple2" => Some(Self::Purple2),
      "pink" => Some(Self::Pink),
      "gray" => Some(Self::Gray),
      _ => None,
    }
  }
}

/// Liquid glass amount with typed handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassAmount {
  Much,
  Glass,
  Less,
}

impl GlassAmount {
  pub fn as_str(&self) -> &'static str {
    match self {
      Self::Much => "much",
      Self::Glass => "glass",
      Self::Less => "less",
    }
  }

  pub fn from_str(raw: &str) -> Option<Self> {
    match raw {
      "much" => Some(Self::Much),
      "glass" => Some(Self::Glass),
      "less" => Some(Self::Less),
      _ => None,
    }
  }
}

/// Effective customization: stored values overlaid on the defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomizeSettings {
  pub wallpaper: String,
  pub accent: String,
  pub theme: String,
  pub glass: String,
  pub revision: u64,
}

impl CustomizeSettings {
  /// JSON shape for `customize_get` / `customize_set` replies.
  pub fn to_json_value(&self) -> JsonValue {
    JsonValue::Object(vec![
      ("wallpaper".to_string(), JsonValue::Str(self.wallpaper.clone())),
      ("accent".to_string(), JsonValue::Str(self.accent.clone())),
      ("theme".to_string(), JsonValue::Str(self.theme.clone())),
      ("glass".to_string(), JsonValue::Str(self.glass.clone())),
      ("revision".to_string(), JsonValue::Integer(self.revision as i64)),
    ])
  }
}

impl Default for CustomizeSettings {
  fn default() -> Self {
    Self {
      wallpaper: DEFAULT_WALLPAPER.to_string(),
      accent: DEFAULT_ACCENT.to_string(),
      theme: DEFAULT_THEME.to_string(),
      glass: DEFAULT_GLASS.to_string(),
      revision: 0,
    }
  }
}

impl CustomizeSettings {
  pub fn theme_mode(&self) -> ThemeMode {
    ThemeMode::from_str(&self.theme).unwrap_or(ThemeMode::Dark)
  }

  pub fn accent_color(&self) -> AccentColor {
    AccentColor::from_str(&self.accent).unwrap_or(AccentColor::Multicolor)
  }

  pub fn glass_amount(&self) -> GlassAmount {
    GlassAmount::from_str(&self.glass).unwrap_or(GlassAmount::Glass)
  }
}

fn valid_accent(value: &str) -> bool {
  ACCENTS.contains(&value)
}

fn valid_theme(value: &str) -> bool {
  THEMES.contains(&value)
}

fn valid_glass(value: &str) -> bool {
  GLASS_AMOUNTS.contains(&value)
}

/// Read the effective settings: stored values win when present and valid,
/// otherwise the defaults apply.
pub fn get(store: &SettingsStore) -> CustomizeSettings {
  let mut settings = CustomizeSettings::default();
  if let Some(value) = store.get(DOMAIN, KEY_WALLPAPER).and_then(|v| v.as_str()) {
    if !value.is_empty() {
      settings.wallpaper = value.to_string();
    }
  }
  if let Some(value) = store.get(DOMAIN, KEY_ACCENT).and_then(|v| v.as_str()) {
    if valid_accent(value) {
      settings.accent = value.to_string();
    }
  }
  if let Some(value) = store.get(DOMAIN, KEY_THEME).and_then(|v| v.as_str()) {
    if valid_theme(value) {
      settings.theme = value.to_string();
    }
  }
  if let Some(value) = store.get(DOMAIN, KEY_GLASS).and_then(|v| v.as_str()) {
    if valid_glass(value) {
      settings.glass = value.to_string();
    }
  }
  if let Some(value) = store.get(DOMAIN, KEY_REVISION).and_then(|v| v.as_u64()) {
    settings.revision = value;
  }
  settings
}

/// Apply a partial update (`None` leaves the key untouched), validate,
/// persist and return the effective settings.
///
/// Returns `Err` without touching the store when any provided value is
/// invalid, or when the store cannot be locked or saved.
pub fn set(
  store: &Arc<Mutex<SettingsStore>>,
  wallpaper: Option<&str>,
  accent: Option<&str>,
  theme: Option<&str>,
  glass: Option<&str>,
) -> Result<CustomizeSettings, String> {
  if let Some(value) = wallpaper {
    if value.is_empty() {
      return Err("customize set failed: wallpaper must not be empty".to_string());
    }
  }
  if let Some(value) = accent {
    if !valid_accent(value) {
      return Err(format!("customize set failed: unknown accent {:?}", value));
    }
  }
  if let Some(value) = theme {
    if !valid_theme(value) {
      return Err(format!("customize set failed: unknown theme {:?}", value));
    }
  }
  if let Some(value) = glass {
    if !valid_glass(value) {
      return Err(format!("customize set failed: unknown glass {:?}", value));
    }
  }
  let mut guard = store
    .lock()
    .map_err(|_| "customize set failed: store is locked".to_string())?;
  if let Some(value) = wallpaper {
    guard.set(DOMAIN, KEY_WALLPAPER, JsonValue::Str(value.to_string()));
  }
  if let Some(value) = accent {
    guard.set(DOMAIN, KEY_ACCENT, JsonValue::Str(value.to_string()));
  }
  if let Some(value) = theme {
    guard.set(DOMAIN, KEY_THEME, JsonValue::Str(value.to_string()));
  }
  if let Some(value) = glass {
    guard.set(DOMAIN, KEY_GLASS, JsonValue::Str(value.to_string()));
  }
  let revision = get(&guard)
    .revision
    .checked_add(1)
    .unwrap_or(u64::MAX);
  guard.set(DOMAIN, KEY_REVISION, JsonValue::Integer(revision as i64));
  let effective = get(&guard);
  guard
    .save()
    .map_err(|e| format!("customize set failed: store save failed: {}", e))?;
  Ok(effective)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  fn memory_store() -> Arc<Mutex<SettingsStore>> {
    Arc::new(Mutex::new(SettingsStore::new(PathBuf::from(
      "/tmp/tontoo-settings-customize-test.json",
    ))))
  }

  #[test]
  fn defaults_when_store_empty() {
    let store = memory_store();
    let guard = store.lock().unwrap();
    assert_eq!(get(&guard), CustomizeSettings::default());
  }

  #[test]
  fn invalid_stored_values_fall_back_to_defaults() {
    let store = memory_store();
    {
      let mut guard = store.lock().unwrap();
      guard.set(DOMAIN, KEY_ACCENT, JsonValue::Str("neon".to_string()));
      guard.set(DOMAIN, KEY_THEME, JsonValue::Str("sepia".to_string()));
      guard.set(DOMAIN, KEY_WALLPAPER, JsonValue::Str(String::new()));
      assert_eq!(get(&guard), CustomizeSettings::default());
    }
  }

  #[test]
  fn stored_values_win_when_valid() {
    let store = memory_store();
    {
      let mut guard = store.lock().unwrap();
      guard.set(DOMAIN, KEY_WALLPAPER, JsonValue::Str("SONOMA".to_string()));
      guard.set(DOMAIN, KEY_ACCENT, JsonValue::Str("blue".to_string()));
      guard.set(DOMAIN, KEY_THEME, JsonValue::Str("light".to_string()));
    }
    let guard = store.lock().unwrap();
    assert_eq!(
      get(&guard),
      CustomizeSettings {
        wallpaper: "SONOMA".to_string(),
        accent: "blue".to_string(),
        theme: "light".to_string(),
        glass: DEFAULT_GLASS.to_string(),
        revision: 0,
      }
    );
  }

  #[test]
  fn set_rejects_invalid_values_without_touching_store() {
    let store = memory_store();
    assert!(set(&store, Some(""), None, None, None).is_err());
    assert!(set(&store, None, Some("neon"), None, None).is_err());
    assert!(set(&store, None, None, Some("sepia"), None).is_err());
    assert!(set(&store, None, None, None, Some("fog")).is_err());
    let guard = store.lock().unwrap();
    assert_eq!(get(&guard), CustomizeSettings::default());
  }

  #[test]
  fn set_applies_partial_updates() {
    let _ = std::fs::remove_file("/tmp/tontoo-settings-customize-test.json");
    let store = memory_store();
    let applied = set(&store, Some("VENTURA"), None, Some("light"), None).unwrap();
    assert_eq!(applied.wallpaper, "VENTURA");
    assert_eq!(applied.accent, DEFAULT_ACCENT);
    assert_eq!(applied.theme, "light");
    assert_eq!(applied.glass, DEFAULT_GLASS);
    assert_eq!(applied.revision, 1);
    let applied = set(&store, None, Some("multicolor"), None, Some("less")).unwrap();
    assert_eq!(applied.revision, 2);
    assert_eq!(applied.accent_color(), AccentColor::Multicolor);
    assert_eq!(applied.theme_mode(), ThemeMode::Light);
    assert_eq!(applied.glass_amount(), GlassAmount::Less);
    let _ = std::fs::remove_file("/tmp/tontoo-settings-customize-test.json");
  }

  #[test]
  fn typed_glass_amounts() {
    for name in GLASS_AMOUNTS {
      assert!(GlassAmount::from_str(name).is_some(), "missing {name}");
      assert_eq!(GlassAmount::from_str(name).unwrap().as_str(), *name);
    }
    assert_eq!(GlassAmount::from_str("fog"), None);
    assert_eq!(CustomizeSettings::default().glass_amount(), GlassAmount::Glass);
  }

  #[test]
  fn typed_accents_cover_settings_palette() {
    for name in ACCENTS {
      assert!(AccentColor::from_str(name).is_some(), "missing {name}");
      assert_eq!(AccentColor::from_str(name).unwrap().as_str(), *name);
    }
    assert_eq!(AccentColor::from_str("neon"), None);
    assert_eq!(ThemeMode::from_str("dark"), Some(ThemeMode::Dark));
    assert_eq!(ThemeMode::from_str("sepia"), None);
  }
}
