use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// In-memory settings store with JSON file persistence.
///
/// Layout on disk is a two-level object:
///
/// ```json
/// {
///   "appearance": { "theme": "dark" },
///   "locale": { "lang": "en_us" }
/// }
/// ```
///
/// Basis only: no watchers, no per-user overlay, no transactions yet.
/// Those are roadmap items, see `wiki/Store.md`.
#[derive(Debug, Default)]
pub struct SettingsStore {
  path: PathBuf,
  domains: HashMap<String, HashMap<String, Value>>,
}

impl SettingsStore {
  pub fn new(path: PathBuf) -> Self {
    Self {
      path,
      domains: HashMap::new(),
    }
  }

  pub fn path(&self) -> &Path {
    &self.path
  }

  /// Load store from disk. Missing file means empty store, invalid JSON
  /// returns `Err` and leaves the current content untouched.
  pub fn load(&mut self) -> io::Result<()> {
    if !self.path.exists() {
      self.domains.clear();
      return Ok(());
    }
    let raw = std::fs::read_to_string(&self.path)?;
    let parsed: HashMap<String, HashMap<String, Value>> =
      serde_json::from_str(&raw).map_err(io::Error::other)?;
    self.domains = parsed;
    Ok(())
  }

  /// Persist store to disk, creating parent directories as needed.
  pub fn save(&self) -> io::Result<()> {
    if let Some(parent) = self.path.parent() {
      std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(&self.domains).map_err(io::Error::other)?;
    std::fs::write(&self.path, raw)?;
    Ok(())
  }

  /// Returns `None` when the domain or key does not exist.
  pub fn get(&self, domain: &str, key: &str) -> Option<&Value> {
    self.domains.get(domain)?.get(key)
  }

  pub fn set(&mut self, domain: &str, key: &str, value: Value) {
    self
      .domains
      .entry(domain.to_string())
      .or_default()
      .insert(key.to_string(), value);
  }

  /// Returns `true` when a value was removed.
  pub fn remove(&mut self, domain: &str, key: &str) -> bool {
    let removed = self
      .domains
      .get_mut(domain)
      .map(|keys| keys.remove(key).is_some())
      .unwrap_or(false);
    if removed {
      if let Some(keys) = self.domains.get(domain) {
        if keys.is_empty() {
          self.domains.remove(domain);
        }
      }
    }
    removed
  }

  pub fn domains(&self) -> Vec<String> {
    let mut names: Vec<String> = self.domains.keys().cloned().collect();
    names.sort();
    names
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn set_get_remove_roundtrip() {
    let mut store = SettingsStore::new(PathBuf::from("/tmp/tontoo-settings-test.json"));
    assert_eq!(store.get("appearance", "theme"), None);
    store.set("appearance", "theme", json!("dark"));
    assert_eq!(store.get("appearance", "theme"), Some(&json!("dark")));
    assert!(store.remove("appearance", "theme"));
    assert_eq!(store.get("appearance", "theme"), None);
  }

  #[test]
  fn save_load_roundtrip() {
    let dir = std::env::temp_dir().join("tontoo-settings-daemon-test");
    let path = dir.join("settings.json");
    let _ = std::fs::remove_file(&path);
    let mut store = SettingsStore::new(path.clone());
    store.set("locale", "lang", json!("en_us"));
    store.save().unwrap();
    let mut reloaded = SettingsStore::new(path.clone());
    reloaded.load().unwrap();
    assert_eq!(reloaded.get("locale", "lang"), Some(&json!("en_us")));
    let _ = std::fs::remove_file(&path);
  }
}
