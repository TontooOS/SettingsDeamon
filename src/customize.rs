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

use serde::{Deserialize, Serialize};

use crate::store::SettingsStore;

/// Store domain holding the customization keys.
pub const DOMAIN: &str = "customize";
/// Wallpaper pack id (e.g. `"THAOELAKE"`).
pub const KEY_WALLPAPER: &str = "wallpaper";
/// Accent color name (one of `ACCENTS`).
pub const KEY_ACCENT: &str = "accent";
/// Color theme (one of `THEMES`).
pub const KEY_THEME: &str = "theme";

pub const OP_CUSTOMIZE_GET: &str = "customize_get";
pub const OP_CUSTOMIZE_SET: &str = "customize_set";

pub const DEFAULT_WALLPAPER: &str = "THAOELAKE";
pub const DEFAULT_ACCENT: &str = "orange";
pub const DEFAULT_THEME: &str = "dark";

/// Accent colors accepted by `customize_set`.
pub const ACCENTS: &[&str] = &["orange", "blue", "green", "purple"];
/// Themes accepted by `customize_set`.
pub const THEMES: &[&str] = &["dark", "light"];

/// Effective customization: stored values overlaid on the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomizeSettings {
  pub wallpaper: String,
  pub accent: String,
  pub theme: String,
}

impl Default for CustomizeSettings {
  fn default() -> Self {
    Self {
      wallpaper: DEFAULT_WALLPAPER.to_string(),
      accent: DEFAULT_ACCENT.to_string(),
      theme: DEFAULT_THEME.to_string(),
    }
  }
}

fn valid_accent(value: &str) -> bool {
  ACCENTS.contains(&value)
}

fn valid_theme(value: &str) -> bool {
  THEMES.contains(&value)
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
  let mut guard = store
    .lock()
    .map_err(|_| "customize set failed: store is locked".to_string())?;
  if let Some(value) = wallpaper {
    guard.set(DOMAIN, KEY_WALLPAPER, serde_json::json!(value));
  }
  if let Some(value) = accent {
    guard.set(DOMAIN, KEY_ACCENT, serde_json::json!(value));
  }
  if let Some(value) = theme {
    guard.set(DOMAIN, KEY_THEME, serde_json::json!(value));
  }
  let effective = get(&guard);
  guard
    .save()
    .map_err(|e| format!("customize set failed: store save failed: {}", e))?;
  Ok(effective)
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;
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
      guard.set(DOMAIN, KEY_ACCENT, json!("neon"));
      guard.set(DOMAIN, KEY_THEME, json!("sepia"));
      guard.set(DOMAIN, KEY_WALLPAPER, json!(""));
      assert_eq!(get(&guard), CustomizeSettings::default());
    }
  }

  #[test]
  fn stored_values_win_when_valid() {
    let store = memory_store();
    {
      let mut guard = store.lock().unwrap();
      guard.set(DOMAIN, KEY_WALLPAPER, json!("SONOMA"));
      guard.set(DOMAIN, KEY_ACCENT, json!("blue"));
      guard.set(DOMAIN, KEY_THEME, json!("light"));
    }
    let guard = store.lock().unwrap();
    assert_eq!(
      get(&guard),
      CustomizeSettings {
        wallpaper: "SONOMA".to_string(),
        accent: "blue".to_string(),
        theme: "light".to_string(),
      }
    );
  }

  #[test]
  fn set_rejects_invalid_values_without_touching_store() {
    let store = memory_store();
    assert!(set(&store, Some(""), None, None).is_err());
    assert!(set(&store, None, Some("neon"), None).is_err());
    assert!(set(&store, None, None, Some("sepia")).is_err());
    let guard = store.lock().unwrap();
    assert_eq!(get(&guard), CustomizeSettings::default());
  }

  #[test]
  fn set_applies_partial_updates() {
    let _ = std::fs::remove_file("/tmp/tontoo-settings-customize-test.json");
    let store = memory_store();
    let applied = set(&store, Some("VENTURA"), None, Some("light")).unwrap();
    assert_eq!(applied.wallpaper, "VENTURA");
    assert_eq!(applied.accent, DEFAULT_ACCENT);
    assert_eq!(applied.theme, "light");
    let _ = std::fs::remove_file("/tmp/tontoo-settings-customize-test.json");
  }
}
