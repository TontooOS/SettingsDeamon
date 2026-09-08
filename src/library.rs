use std::collections::HashMap;

/// One managed settings library (future dynamic module).
#[derive(Debug, Clone)]
pub struct LibraryEntry {
  pub name: String,
  pub version: String,
}

/// Registry for settings libraries.
///
/// Basis only: plain in-memory registry. Later this owns discovery,
/// validation, loading/unloading and version negotiation for settings
/// modules. Socket-triggered reload is a roadmap item, see
/// `wiki/Library.md`.
#[derive(Debug, Default)]
pub struct LibraryManager {
  libs: HashMap<String, LibraryEntry>,
}

impl LibraryManager {
  pub fn new() -> Self {
    Self {
      libs: HashMap::new(),
    }
  }

  /// Register or replace a library entry.
  pub fn register(&mut self, name: &str, version: &str) {
    self.libs.insert(
      name.to_string(),
      LibraryEntry {
        name: name.to_string(),
        version: version.to_string(),
      },
    );
  }

  /// Returns `true` when an entry was removed.
  pub fn unregister(&mut self, name: &str) -> bool {
    self.libs.remove(name).is_some()
  }

  pub fn is_registered(&self, name: &str) -> bool {
    self.libs.contains_key(name)
  }

  pub fn list(&self) -> Vec<LibraryEntry> {
    let mut entries: Vec<LibraryEntry> = self.libs.values().cloned().collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn register_list_unregister() {
    let mut manager = LibraryManager::new();
    manager.register("appearance", "0.1.0");
    manager.register("locale", "0.1.0");
    assert!(manager.is_registered("appearance"));
    assert_eq!(manager.list().len(), 2);
    assert!(manager.unregister("appearance"));
    assert!(!manager.is_registered("appearance"));
  }
}
