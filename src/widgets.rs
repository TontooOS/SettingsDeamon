//! Desktop widget registry.
//!
//! Apps register their widget descriptors once per start and then push content
//! trees. The desktop widget engine reads the list and subscribes to content
//! changes, so it never has to know which apps are installed.
//!
//! Descriptors are persisted in the settings store under the `widgets` domain,
//! one entry per widget id, because the registry has to survive a daemon
//! restart: the user starts an app once, and its widgets stay available.
//! Content trees are **not** persisted. A content tree is a snapshot of
//! "right now" (a clock reading, a battery level); replaying a stale one after
//! a reboot would show wrong data, so the engine repaints as soon as the owning
//! app pushes again.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::JsonValue;
use crate::store::SettingsStore;

/// Settings store domain holding one descriptor per widget id.
pub const DOMAIN: &str = "widgets";

/// Op that registers or replaces every widget of one app.
pub const OP_REGISTER: &str = "widget_register";
/// Op that lists every registered widget.
pub const OP_LIST: &str = "widget_list";
/// Op that drops every widget of one app.
pub const OP_UNREGISTER: &str = "widget_unregister";
/// Op that pushes the content tree of one widget.
pub const OP_CONTENT: &str = "widget_content";
/// Op that returns the current content tree of every widget.
pub const OP_CONTENT_LIST: &str = "widget_content_list";

/// Event pushed after a content tree changed.
pub const EVENT_CONTENT_CHANGED: &str = "widget_content_changed";
/// Event pushed after the set of registered widgets changed.
pub const EVENT_WIDGETS_CHANGED: &str = "widgets_changed";

/// Live content trees, keyed by widget id.
fn content_store() -> &'static Mutex<HashMap<String, JsonValue>> {
  static STORE: OnceLock<Mutex<HashMap<String, JsonValue>>> = OnceLock::new();
  STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Seconds since the unix epoch, `0` when the clock is before it.
fn now() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs() as i64)
    .unwrap_or(0)
}

/// Validate one descriptor object and return its widget id.
///
/// An error message is returned instead of a value when `id` is missing or
/// empty, when `name` is missing, or when `sizes` is empty or holds no known
/// size name.
pub fn descriptor_id(descriptor: &JsonValue) -> Result<String, String> {
  let id = descriptor
    .get("id")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .trim()
    .to_string();
  if id.is_empty() {
    return Err("widget id must not be empty".to_string());
  }
  if descriptor
    .get("name")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .trim()
    .is_empty()
  {
    return Err(format!("widget {id} has no name"));
  }
  let sizes = match descriptor.get("sizes") {
    Some(JsonValue::Array(items)) => items,
    _ => return Err(format!("widget {id} has no sizes array")),
  };
  if sizes.is_empty() {
    return Err(format!("widget {id} has no sizes"));
  }
  let known = sizes.iter().filter(|item| match item.as_str() {
    Some("small") | Some("medium") | Some("large") => true,
    _ => false,
  });
  if known.count() == 0 {
    return Err(format!("widget {id} supports no known size"));
  }
  Ok(id)
}

/// Register or replace every widget of `app`.
///
/// The whole batch is validated before anything is written, so a malformed
/// descriptor leaves the registry untouched. Widgets the app registered in an
/// earlier run but did not register again are dropped, and their content trees
/// are discarded. Returns the number of widgets now registered for `app`.
pub fn register(
  store: &mut SettingsStore,
  app: &str,
  descriptors: &[JsonValue],
) -> Result<usize, String> {
  let app = app.trim();
  if app.is_empty() {
    return Err("widget register failed: app must not be empty".to_string());
  }
  if descriptors.is_empty() {
    return Err("widget register failed: no descriptors".to_string());
  }

  // Validate the whole batch first so a bad entry cannot half-apply.
  let mut prepared: Vec<(String, JsonValue)> = Vec::with_capacity(descriptors.len());
  for descriptor in descriptors {
    let id = descriptor_id(descriptor)?;
    let mut entry = match descriptor {
      JsonValue::Object(entries) => entries.clone(),
      other => vec![("id".to_string(), other.clone())],
    };
    entry.retain(|(key, _)| key != "app" && key != "registered_at");
    entry.insert(0, ("app".to_string(), JsonValue::Str(app.to_string())));
    entry.push(("registered_at".to_string(), JsonValue::Integer(now())));
    prepared.push((id, JsonValue::Object(entry)));
  }

  // Drop widgets this app no longer offers.
  let stale: Vec<String> = store
    .keys(DOMAIN)
    .into_iter()
    .filter(|key| {
      store
        .get(DOMAIN, key)
        .and_then(|value| value.get("app"))
        .and_then(|v| v.as_str())
        .is_some_and(|owner| owner == app)
        && !prepared.iter().any(|(id, _)| id == key)
    })
    .collect();
  for id in stale {
    store.remove(DOMAIN, &id);
    drop_content(&id);
  }

  for (id, entry) in prepared {
    store.set(DOMAIN, &id, entry);
  }

  store
    .save()
    .map_err(|e| format!("widget register failed: store save failed: {e}"))?;
  Ok(descriptors.len())
}

/// Every registered widget, sorted by app bundle id then widget id.
pub fn list(store: &SettingsStore) -> Vec<JsonValue> {
  let mut entries: Vec<JsonValue> = store
    .keys(DOMAIN)
    .into_iter()
    .filter_map(|key| store.get(DOMAIN, &key).cloned())
    .collect();
  entries.sort_by(|a, b| {
    let key = |value: &JsonValue| {
      (
        value.get("app").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        value.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
      )
    };
    key(a).cmp(&key(b))
  });
  entries
}

/// Registered widgets grouped by owning app, for the edit panel sidebar.
///
/// Keys are app bundle ids sorted alphabetically; values are the widgets of
/// that app in registry order. Apps with no registered widget are absent.
pub fn list_by_app(store: &SettingsStore) -> Vec<(String, Vec<JsonValue>)> {
  let mut grouped: Vec<(String, Vec<JsonValue>)> = Vec::new();
  for entry in list(store) {
    let app = entry
      .get("app")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string();
    match grouped.iter_mut().find(|(name, _)| *name == app) {
      Some((_, widgets)) => widgets.push(entry),
      None => grouped.push((app, vec![entry])),
    }
  }
  grouped
}

/// Drop every widget of `app`, including its content trees.
///
/// Returns how many widgets were removed.
pub fn unregister(store: &mut SettingsStore, app: &str) -> Result<usize, String> {
  let app = app.trim();
  let ids: Vec<String> = store
    .keys(DOMAIN)
    .into_iter()
    .filter(|key| {
      store
        .get(DOMAIN, key)
        .and_then(|value| value.get("app"))
        .and_then(|v| v.as_str())
        .is_some_and(|owner| owner == app)
    })
    .collect();

  for id in &ids {
    store.remove(DOMAIN, id);
    drop_content(id);
  }
  if ids.is_empty() {
    return Ok(0);
  }
  store
    .save()
    .map_err(|e| format!("widget unregister failed: store save failed: {e}"))?;
  Ok(ids.len())
}

/// Store the content tree of one widget.
///
/// Returns `Ok(false)` when `widget_id` is not registered, which happens when
/// the app pushed content before registering.
pub fn set_content(widget_id: &str, content: JsonValue) -> Result<bool, String> {
  let widget_id = widget_id.trim();
  if widget_id.is_empty() {
    return Err("widget content failed: widget id must not be empty".to_string());
  }
  if content.get("n").and_then(|v| v.as_str()).is_none() {
    return Err(format!(
      "widget content failed: {widget_id} has no node name"
    ));
  }
  let mut guard = content_store()
    .lock()
    .map_err(|_| "widget content failed: content store is poisoned".to_string())?;
  guard.insert(widget_id.to_string(), content);
  Ok(true)
}

/// Current content trees, sorted by widget id.
pub fn content_list() -> Vec<(String, JsonValue)> {
  let Ok(guard) = content_store().lock() else {
    return Vec::new();
  };
  let mut entries: Vec<(String, JsonValue)> = guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
  entries.sort_by(|a, b| a.0.cmp(&b.0));
  entries
}

/// Current content tree of one widget.
pub fn content_of(widget_id: &str) -> Option<JsonValue> {
  content_store().lock().ok()?.get(widget_id).cloned()
}

/// Discard the content tree of one widget. Missing ids are ignored.
pub fn drop_content(widget_id: &str) {
  if let Ok(mut guard) = content_store().lock() {
    guard.remove(widget_id);
  }
}

/// Payload for [`EVENT_CONTENT_CHANGED`].
pub fn content_event(widget_id: &str, content: &JsonValue) -> JsonValue {
  JsonValue::Object(vec![
    ("widget".to_string(), JsonValue::Str(widget_id.to_string())),
    ("content".to_string(), content.clone()),
  ])
}

/// Payload for [`EVENT_WIDGETS_CHANGED`], carrying the new full registry.
pub fn widgets_event(entries: &[JsonValue]) -> JsonValue {
  JsonValue::Object(vec![
    ("widgets".to_string(), JsonValue::Array(entries.to_vec())),
    ("count".to_string(), JsonValue::Integer(entries.len() as i64)),
  ])
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Content trees live in one process wide map, so the tests that touch them
  /// run one after another and use per-test widget ids.
  fn content_guard() -> std::sync::MutexGuard<'static, ()> {
    match crate::TEST_ENV_LOCK.lock() {
      Ok(guard) => guard,
      Err(poisoned) => poisoned.into_inner(),
    }
  }

  /// Widget id unique to one test, so a stale entry from an earlier run can
  /// never make an assertion pass or fail by accident.
  fn unique_id(tag: &str) -> String {
    format!(
      "com.test.{tag}.{}",
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
    )
  }

  fn temp_store(tag: &str) -> (tempdir::TempDir, SettingsStore) {
    let dir = tempdir::TempDir::new(tag).expect("temp dir");
    let path = dir.path().join("settings.json");
    let store = SettingsStore::new(path);
    (dir, store)
  }

  fn descriptor(id: &str, name: &str, sizes: &[&str]) -> JsonValue {
    JsonValue::Object(vec![
      ("id".to_string(), JsonValue::Str(id.to_string())),
      ("name".to_string(), JsonValue::Str(name.to_string())),
      (
        "sizes".to_string(),
        JsonValue::Array(sizes.iter().map(|s| JsonValue::Str((*s).to_string())).collect()),
      ),
      (
        "default_size".to_string(),
        JsonValue::Str(sizes.first().copied().unwrap_or("medium").to_string()),
      ),
    ])
  }

  fn sample_content() -> JsonValue {
    JsonValue::Object(vec![("n".to_string(), JsonValue::Str("text".to_string()))])
  }

  #[test]
  fn a_valid_descriptor_reports_its_id() {
    let d = descriptor("com.a.b", "B", &["small", "medium"]);
    assert_eq!(descriptor_id(&d).unwrap(), "com.a.b");
  }

  #[test]
  fn an_empty_id_is_rejected() {
    let d = descriptor("   ", "B", &["small"]);
    assert!(descriptor_id(&d).unwrap_err().contains("id"));
  }

  #[test]
  fn a_missing_name_is_rejected() {
    let d = JsonValue::Object(vec![("id".to_string(), JsonValue::Str("a.b".to_string()))]);
    assert!(descriptor_id(&d).unwrap_err().contains("name"));
  }

  #[test]
  fn an_empty_size_list_is_rejected() {
    let d = descriptor("com.a.b", "B", &[]);
    assert!(descriptor_id(&d).unwrap_err().contains("sizes"));
  }

  #[test]
  fn unknown_size_names_are_rejected() {
    let d = descriptor("com.a.b", "B", &["huge"]);
    assert!(descriptor_id(&d).unwrap_err().contains("known size"));
  }

  #[test]
  fn one_known_size_among_unknown_ones_is_enough() {
    let d = descriptor("com.a.b", "B", &["huge", "large"]);
    assert_eq!(descriptor_id(&d).unwrap(), "com.a.b");
  }

  #[test]
  fn registering_persists_and_lists() {
    let (_dir, mut store) = temp_store("reg");
    let count = register(
      &mut store,
      "com.a.app",
      &[descriptor("com.a.one", "One", &["small"]), descriptor("com.a.two", "Two", &["large"])],
    )
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(list(&store).len(), 2);
  }

  #[test]
  fn registering_again_replaces_and_drops_stale_entries() {
    let (_dir, mut store) = temp_store("replace");
    register(
      &mut store,
      "com.a.app",
      &[descriptor("com.a.one", "One", &["small"]), descriptor("com.a.two", "Two", &["small"])],
    )
    .unwrap();
    // Second run only offers one widget.
    register(&mut store, "com.a.app", &[descriptor("com.a.one", "One", &["small"])]).unwrap();
    let ids: Vec<String> = list(&store)
      .iter()
      .map(|e| e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string())
      .collect();
    assert_eq!(ids, vec!["com.a.one".to_string()]);
  }

  #[test]
  fn a_bad_entry_leaves_the_registry_untouched() {
    let (_dir, mut store) = temp_store("atomic");
    register(&mut store, "com.a.app", &[descriptor("com.a.one", "One", &["small"])]).unwrap();
    let err = register(
      &mut store,
      "com.a.other",
      &[descriptor("com.a.two", "Two", &["small"]), descriptor("", "Bad", &["small"])],
    )
    .unwrap_err();
    assert!(err.contains("id"));
    assert_eq!(list(&store).len(), 1);
  }

  #[test]
  fn an_empty_batch_is_rejected() {
    let (_dir, mut store) = temp_store("empty");
    assert!(register(&mut store, "com.a.app", &[]).unwrap_err().contains("no descriptors"));
  }

  #[test]
  fn an_empty_app_is_rejected() {
    let (_dir, mut store) = temp_store("noapp");
    assert!(
      register(&mut store, "  ", &[descriptor("a.b", "B", &["small"])])
        .unwrap_err()
        .contains("app")
    );
  }

  #[test]
  fn unregistered_widgets_disappear() {
    let (_dir, mut store) = temp_store("unreg");
    register(&mut store, "com.a.app", &[descriptor("com.a.one", "One", &["small"])]).unwrap();
    register(&mut store, "com.b.app", &[descriptor("com.b.one", "One", &["small"])]).unwrap();
    assert_eq!(unregister(&mut store, "com.a.app").unwrap(), 1);
    let remaining = list(&store);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].get("app").and_then(|v| v.as_str()), Some("com.b.app"));
  }

  #[test]
  fn unregistering_an_unknown_app_removes_nothing() {
    let (_dir, mut store) = temp_store("unreg-none");
    register(&mut store, "com.a.app", &[descriptor("com.a.one", "One", &["small"])]).unwrap();
    assert_eq!(unregister(&mut store, "com.zzz").unwrap(), 0);
    assert_eq!(list(&store).len(), 1);
  }

  #[test]
  fn entries_carry_their_app_and_registration_time() {
    let (_dir, mut store) = temp_store("meta");
    register(&mut store, "com.a.app", &[descriptor("com.a.one", "One", &["small"])]).unwrap();
    let entry = &list(&store)[0];
    assert_eq!(entry.get("app").and_then(|v| v.as_str()), Some("com.a.app"));
    assert!(entry.get("registered_at").and_then(|v| v.as_i64()).unwrap_or(0) > 0);
  }

  #[test]
  fn the_list_is_sorted_by_app_then_id() {
    let (_dir, mut store) = temp_store("sorted");
    register(&mut store, "com.b.app", &[descriptor("com.b.z", "Z", &["small"])]).unwrap();
    register(
      &mut store,
      "com.a.app",
      &[descriptor("com.a.b", "B", &["small"]), descriptor("com.a.a", "A", &["small"])],
    )
    .unwrap();
    let ids: Vec<String> = list(&store)
      .iter()
      .map(|e| e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string())
      .collect();
    assert_eq!(ids, vec!["com.a.a", "com.a.b", "com.b.z"]);
  }

  #[test]
  fn widgets_group_by_owning_app() {
    let (_dir, mut store) = temp_store("group");
    register(
      &mut store,
      "com.a.app",
      &[descriptor("com.a.one", "One", &["small"]), descriptor("com.a.two", "Two", &["small"])],
    )
    .unwrap();
    register(&mut store, "com.b.app", &[descriptor("com.b.one", "One", &["small"])]).unwrap();
    let grouped = list_by_app(&store);
    assert_eq!(grouped.len(), 2);
    assert_eq!(grouped[0].0, "com.a.app");
    assert_eq!(grouped[0].1.len(), 2);
    assert_eq!(grouped[1].0, "com.b.app");
    assert_eq!(grouped[1].1.len(), 1);
  }

  #[test]
  fn content_is_stored_and_read_back() {
    let _guard = content_guard();
    let id = unique_id("store");
    let content = JsonValue::Object(vec![
      ("n".to_string(), JsonValue::Str("text".to_string())),
      ("text".to_string(), JsonValue::Str("12:34".to_string())),
    ]);
    assert!(set_content(&id, content.clone()).unwrap());
    assert_eq!(content_of(&id).unwrap(), content);
  }

  #[test]
  fn content_without_a_node_name_is_rejected() {
    let _guard = content_guard();
    let bad = JsonValue::Object(vec![("text".to_string(), JsonValue::Str("x".to_string()))]);
    assert!(set_content(&unique_id("noname"), bad).unwrap_err().contains("node name"));
  }

  #[test]
  fn content_with_an_empty_widget_id_is_rejected() {
    let _guard = content_guard();
    assert!(set_content("  ", JsonValue::Object(Vec::new())).unwrap_err().contains("id"));
  }

  #[test]
  fn dropping_content_removes_it() {
    let _guard = content_guard();
    let id = unique_id("drop");
    set_content(&id, sample_content()).unwrap();
    drop_content(&id);
    assert!(content_of(&id).is_none());
    // Missing ids are ignored.
    drop_content(&unique_id("never"));
  }

  #[test]
  fn unregistering_an_app_discards_its_content() {
    let _guard = content_guard();
    let (_dir, mut store) = temp_store("unreg-content");
    let id = unique_id("owned");
    register(&mut store, "com.a.app", &[descriptor(&id, "One", &["small"])]).unwrap();
    set_content(&id, sample_content()).unwrap();
    unregister(&mut store, "com.a.app").unwrap();
    assert!(content_of(&id).is_none());
  }

  #[test]
  fn dropping_a_stale_widget_discards_its_content() {
    let _guard = content_guard();
    let (_dir, mut store) = temp_store("stale-content");
    let kept = unique_id("kept");
    let dropped = unique_id("dropped");
    register(
      &mut store,
      "com.a.app",
      &[descriptor(&kept, "One", &["small"]), descriptor(&dropped, "Two", &["small"])],
    )
    .unwrap();
    set_content(&dropped, sample_content()).unwrap();
    register(&mut store, "com.a.app", &[descriptor(&kept, "One", &["small"])]).unwrap();
    assert!(content_of(&dropped).is_none());
    assert!(content_of(&kept).is_none(), "kept widget never had content");
  }

  #[test]
  fn content_list_is_sorted_by_widget_id() {
    let _guard = content_guard();
    let first = unique_id("a");
    let second = unique_id("b");
    set_content(&first, sample_content()).unwrap();
    set_content(&second, sample_content()).unwrap();
    let ids: Vec<String> = content_list().into_iter().map(|(id, _)| id).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
    drop_content(&first);
    drop_content(&second);
  }

  #[test]
  fn event_payloads_carry_the_expected_fields() {
    let content = sample_content();
    let payload = content_event("com.a.one", &content);
    assert_eq!(payload.get("widget").and_then(|v| v.as_str()), Some("com.a.one"));
    assert_eq!(payload.get("content"), Some(&content));

    let entries = vec![descriptor("com.a.one", "One", &["small"])];
    let payload = widgets_event(&entries);
    assert_eq!(payload.get("count").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(payload.get("widgets").and_then(|v| v.as_array()).map(Vec::len), Some(1));
  }

  #[test]
  fn op_names_match_the_widgetkit_client() {
    assert_eq!(OP_REGISTER, "widget_register");
    assert_eq!(OP_LIST, "widget_list");
    assert_eq!(OP_UNREGISTER, "widget_unregister");
    assert_eq!(OP_CONTENT, "widget_content");
    assert_eq!(OP_CONTENT_LIST, "widget_content_list");
  }

  mod tempdir {
    /// Minimal scoped temp directory so the tests need no dev-dependency.
    pub struct TempDir(std::path::PathBuf);

    impl TempDir {
      pub fn new(tag: &str) -> std::io::Result<Self> {
        let unique = format!(
          "tontoo-settings-widgets-{}-{}",
          tag,
          std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
      }

      pub fn path(&self) -> &std::path::Path {
        &self.0
      }
    }

    impl Drop for TempDir {
      fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
      }
    }
  }
}