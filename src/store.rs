use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::json::JsonValue;

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
  domains: HashMap<String, HashMap<String, JsonValue>>,
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
    let parsed = JsonValue::parse(&raw).map_err(io::Error::other)?;
    let mut domains: HashMap<String, HashMap<String, JsonValue>> = HashMap::new();
    let entries = parsed.object_entries().ok_or_else(|| {
      io::Error::other("settings store root must be an object".to_string())
    })?;
    for (domain, values) in entries {
      let members = values.object_entries().ok_or_else(|| {
        io::Error::other(format!("domain {:?} must be an object", domain))
      })?;
      let mut map = HashMap::new();
      for (key, value) in members {
        map.insert(key.clone(), value.clone());
      }
      domains.insert(domain.clone(), map);
    }
    self.domains = domains;
    Ok(())
  }

  /// Persist store to disk, creating parent directories as needed.
  pub fn save(&self) -> io::Result<()> {
    if let Some(parent) = self.path.parent() {
      std::fs::create_dir_all(parent)?;
    }
    let mut domains: Vec<(String, JsonValue)> = Vec::new();
    for (domain, keys) in &self.domains {
      let mut members: Vec<(String, JsonValue)> = keys
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
      members.sort_by(|a, b| a.0.cmp(&b.0));
      domains.push((domain.clone(), JsonValue::Object(members)));
    }
    domains.sort_by(|a, b| a.0.cmp(&b.0));
    let raw = JsonValue::Object(domains).stringify(true);
    std::fs::write(&self.path, raw)?;
    Ok(())
  }

  /// Returns `None` when the domain or key does not exist.
  pub fn get(&self, domain: &str, key: &str) -> Option<&JsonValue> {
    self.domains.get(domain)?.get(key)
  }

  pub fn set(&mut self, domain: &str, key: &str, value: JsonValue) {
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

  /// Sorted key names of one domain (empty when the domain is missing).
  pub fn keys(&self, domain: &str) -> Vec<String> {
    let mut names: Vec<String> = self
      .domains
      .get(domain)
      .map(|keys| keys.keys().cloned().collect())
      .unwrap_or_default();
    names.sort();
    names
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::json::JsonValue;

  #[test]
  fn set_get_remove_roundtrip() {
    let mut store = SettingsStore::new(PathBuf::from("/tmp/tontoo-settings-test.json"));
    assert_eq!(store.get("appearance", "theme"), None);
    store.set("appearance", "theme", JsonValue::Str("dark".to_string()));
    assert_eq!(
      store.get("appearance", "theme"),
      Some(&JsonValue::Str("dark".to_string()))
    );
    assert!(store.remove("appearance", "theme"));
    assert_eq!(store.get("appearance", "theme"), None);
  }

  #[test]
  fn keys_lists_sorted_domain_keys() {
    let mut store = SettingsStore::new(PathBuf::from("/tmp/tontoo-settings-test.json"));
    assert!(store.keys("display").is_empty());
    store.set("display", "night_light", JsonValue::Bool(true));
    store.set("display", "brightness", JsonValue::Integer(80));
    assert_eq!(
      store.keys("display"),
      vec!["brightness".to_string(), "night_light".to_string()]
    );
  }

  #[test]
  fn save_load_roundtrip() {
    let dir = std::env::temp_dir().join("tontoo-settings-daemon-test");
    let path = dir.join("settings.json");
    let _ = std::fs::remove_file(&path);
    let mut store = SettingsStore::new(path.clone());
    store.set("locale", "lang", JsonValue::Str("en_us".to_string()));
    store.save().unwrap();
    let mut reloaded = SettingsStore::new(path.clone());
    reloaded.load().unwrap();
    assert_eq!(
      reloaded.get("locale", "lang"),
      Some(&JsonValue::Str("en_us".to_string()))
    );
    let _ = std::fs::remove_file(&path);
  }
}
