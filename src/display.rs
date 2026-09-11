//! Display backend owned by the settings daemon.
//!
//! Brightness (0-100), night light and per-output modes persist in the
//! `display` store domain; live output data (modes, current mode) always
//! comes from the compositor. Socket ops: `display_get` is a public read
//! op, `display_set` is a private write op with no public client
//! library, reserved for the Settings app (`com.tontoo.systemsettings`).
//!
//! `display_set` forwards to the compositor first and persists after
//! (strict like `wallpaper_apply`): nothing is persisted when the
//! compositor is unreachable.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::store::SettingsStore;

/// Store domain holding brightness, night light and per-output modes.
pub const DOMAIN: &str = "display";
pub const KEY_BRIGHTNESS: &str = "brightness";
pub const KEY_NIGHT_LIGHT: &str = "night_light";

pub const OP_DISPLAY_GET: &str = "display_get";
pub const OP_DISPLAY_SET: &str = "display_set";

pub const DEFAULT_BRIGHTNESS: u32 = 100;

/// One output mode: resolution plus refresh rate in Hz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayMode {
  #[serde(default)]
  pub width: i32,
  #[serde(default)]
  pub height: i32,
  #[serde(default)]
  pub refresh: u32,
}

/// One output with its modes and live current mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayOutput {
  #[serde(default)]
  pub name: String,
  #[serde(default)]
  pub modes: Vec<DisplayMode>,
  pub current: Option<DisplayMode>,
}

/// Effective display state: live outputs plus stored settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayState {
  #[serde(default)]
  pub outputs: Vec<DisplayOutput>,
  pub brightness: u32,
  pub night_light: bool,
}

/// Stored brightness (0-100), defaulting to full.
pub fn brightness_in(store: &SettingsStore) -> u32 {
  store
    .get(DOMAIN, KEY_BRIGHTNESS)
    .and_then(|v| v.as_u64())
    .map(|v| v.min(100) as u32)
    .unwrap_or(DEFAULT_BRIGHTNESS)
}

/// Stored night light flag, defaulting to off.
pub fn night_light_in(store: &SettingsStore) -> bool {
  store
    .get(DOMAIN, KEY_NIGHT_LIGHT)
    .and_then(|v| v.as_bool())
    .unwrap_or(false)
}

/// Stored mode for an output, if any.
pub fn stored_mode(store: &SettingsStore, output: &str) -> Option<DisplayMode> {
  let value = store.get(DOMAIN, output)?;
  Some(DisplayMode {
    width: value.get("width")?.as_i64()? as i32,
    height: value.get("height")?.as_i64()? as i32,
    refresh: value.get("refresh")?.as_u64()? as u32,
  })
}

/// Live outputs from the compositor. Required: without the compositor
/// there are no modes to offer.
fn live_outputs() -> Result<Vec<DisplayOutput>, String> {
  let reply = compositor_call(
    serde_json::json!({"op": "get_displays"}),
  )?;
  serde_json::from_value(reply.get("outputs").cloned().unwrap_or(serde_json::Value::Null))
    .map_err(|e| format!("display get failed: outputs invalid: {}", e))
}

/// Effective display state: live outputs plus stored settings.
pub fn get(store: &SettingsStore) -> Result<DisplayState, String> {
  Ok(DisplayState {
    outputs: live_outputs()?,
    brightness: brightness_in(store),
    night_light: night_light_in(store),
  })
}

/// Send one frame to the compositor socket, return the `result`.
fn compositor_call(request: serde_json::Value) -> Result<serde_json::Value, String> {
  use std::io::{BufRead, BufReader, Write};
  use std::time::Duration;

  let sock = crate::wallpaper::compositor_socket();
  let mut stream = std::os::unix::net::UnixStream::connect(&sock).map_err(|e| {
    format!(
      "display failed: compositor unreachable at {}: {}",
      sock.display(),
      e
    )
  })?;
  let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
  let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
  let mut line = request.to_string();
  line.push('\n');
  stream
    .write_all(line.as_bytes())
    .map_err(|e| format!("display failed: compositor write failed: {}", e))?;
  stream
    .flush()
    .map_err(|e| format!("display failed: compositor write failed: {}", e))?;
  let mut reader = BufReader::new(&stream);
  let mut reply = String::new();
  reader
    .read_line(&mut reply)
    .map_err(|e| format!("display failed: compositor read failed: {}", e))?;
  let frame: serde_json::Value = serde_json::from_str(&reply)
    .map_err(|e| format!("display failed: compositor reply invalid: {}", e))?;
  if frame.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
    Ok(frame.get("result").cloned().unwrap_or(serde_json::Value::Null))
  } else {
    Err(format!(
      "display failed: {}",
      frame.get("error").and_then(|v| v.as_str()).unwrap_or("compositor error")
    ))
  }
}

/// Apply partial display settings live and persist them. Brightness must
/// be 0-100, refresh 1-1000 Hz. Forwards to the compositor first;
/// nothing is persisted when it is unreachable. Persists brightness and
/// night light when given, plus the effective mode of the target output.
/// Returns the effective state.
pub fn set(
  store: &Arc<Mutex<SettingsStore>>,
  output: Option<&str>,
  width: Option<i64>,
  height: Option<i64>,
  refresh: Option<u32>,
  brightness: Option<f64>,
  night_light: Option<bool>,
) -> Result<DisplayState, String> {
  if let Some(value) = brightness {
    if !(0.0..=100.0).contains(&value) {
      return Err(format!("display set failed: invalid brightness: {}", value));
    }
  }
  if let Some(value) = refresh {
    if value == 0 || value > 1000 {
      return Err(format!("display set failed: invalid refresh rate: {}", value));
    }
  }
  let mut flat = serde_json::Map::new();
  flat.insert("op".to_string(), serde_json::json!("set_display"));
  if let Some(output) = output {
    if output.is_empty() {
      return Err("display set failed: output must not be empty".to_string());
    }
    flat.insert("output".to_string(), serde_json::json!(output));
  }
  if let Some(width) = width {
    if width <= 0 {
      return Err(format!("display set failed: invalid width: {}", width));
    }
    flat.insert("width".to_string(), serde_json::json!(width));
  }
  if let Some(height) = height {
    if height <= 0 {
      return Err(format!("display set failed: invalid height: {}", height));
    }
    flat.insert("height".to_string(), serde_json::json!(height));
  }
  if let Some(refresh) = refresh {
    flat.insert("refresh".to_string(), serde_json::json!(refresh));
  }
  if let Some(brightness) = brightness {
    flat.insert("brightness".to_string(), serde_json::json!(brightness));
  }
  if let Some(night_light) = night_light {
    flat.insert("night_light".to_string(), serde_json::json!(night_light));
  }
  let result = compositor_call(serde_json::Value::Object(flat))?;
  let mut guard = store
    .lock()
    .map_err(|_| "display set failed: store is locked".to_string())?;
  if let Some(value) = brightness {
    guard.set(DOMAIN, KEY_BRIGHTNESS, serde_json::json!(value.round() as u32));
  }
  if let Some(night_light) = night_light {
    guard.set(DOMAIN, KEY_NIGHT_LIGHT, serde_json::json!(night_light));
  }
  if let (Some(name), Some(mode)) = (
    result.get("output").and_then(|v| v.as_str()),
    result.get("mode"),
  ) {
    if !mode.is_null() {
      guard.set(DOMAIN, name, mode.clone());
    }
  }
  guard
    .save()
    .map_err(|e| format!("display set failed: store save failed: {}", e))?;
  drop(guard);
  let guard = store
    .lock()
    .map_err(|_| "display set failed: store is locked".to_string())?;
  get(&guard)
}

/// Push the stored display settings to the compositor (startup sync,
/// best effort): brightness, night light and every stored output mode.
pub fn push_to_compositor(store: &SettingsStore) -> Result<(), String> {
  let mut flat = serde_json::Map::new();
  flat.insert("op".to_string(), serde_json::json!("set_display"));
  flat.insert(
    "brightness".to_string(),
    serde_json::json!(brightness_in(store) as f64),
  );
  flat.insert(
    "night_light".to_string(),
    serde_json::json!(night_light_in(store)),
  );
  compositor_call(serde_json::Value::Object(flat))?;
  for name in store.keys(DOMAIN) {
    if name == KEY_BRIGHTNESS || name == KEY_NIGHT_LIGHT {
      continue;
    }
    if let Some(mode) = stored_mode(store, &name) {
      let mut flat = serde_json::Map::new();
      flat.insert("op".to_string(), serde_json::json!("set_display"));
      flat.insert("output".to_string(), serde_json::json!(name));
      flat.insert("width".to_string(), serde_json::json!(mode.width));
      flat.insert("height".to_string(), serde_json::json!(mode.height));
      flat.insert("refresh".to_string(), serde_json::json!(mode.refresh));
      compositor_call(serde_json::Value::Object(flat))?;
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;
  use std::path::PathBuf;

  fn memory_store(path: &std::path::Path) -> Arc<Mutex<SettingsStore>> {
    Arc::new(Mutex::new(SettingsStore::new(path.to_path_buf())))
  }

  fn temp_case(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tontoo-display-{}", name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
  }

  #[test]
  fn stored_defaults() {
    let dir = temp_case("defaults");
    let store = SettingsStore::new(dir.join("settings.json"));
    assert_eq!(brightness_in(&store), DEFAULT_BRIGHTNESS);
    assert!(!night_light_in(&store));
    assert!(stored_mode(&store, "HDMI-1").is_none());
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn stored_values_roundtrip() {
    let dir = temp_case("roundtrip");
    let mut store = SettingsStore::new(dir.join("settings.json"));
    store.set(DOMAIN, KEY_BRIGHTNESS, json!(80));
    store.set(DOMAIN, KEY_NIGHT_LIGHT, json!(true));
    store.set(
      DOMAIN,
      "HDMI-1",
      json!({"width": 1920, "height": 1080, "refresh": 120}),
    );
    assert_eq!(brightness_in(&store), 80);
    assert!(night_light_in(&store));
    assert_eq!(
      stored_mode(&store, "HDMI-1"),
      Some(DisplayMode { width: 1920, height: 1080, refresh: 120 })
    );
    // Out-of-range brightness clamps, unknown shapes miss.
    store.set(DOMAIN, KEY_BRIGHTNESS, json!(250));
    assert_eq!(brightness_in(&store), 100);
    store.set(DOMAIN, "DP-1", json!({"width": 1920}));
    assert!(stored_mode(&store, "DP-1").is_none());
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn set_validates_before_touching_anything() {
    let dir = temp_case("validate");
    let store = memory_store(&dir.join("settings.json"));
    assert!(set(&store, None, None, None, Some(0), None, None).is_err());
    assert!(set(&store, None, None, None, Some(1001), None, None).is_err());
    assert!(set(&store, None, None, None, None, Some(101.0), None).is_err());
    assert!(set(&store, None, Some(-800), None, None, None, None).is_err());
    assert!(set(&store, Some(""), None, None, None, None, None).is_err());
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn state_shape_matches_socket_contract() {
    let value = serde_json::to_value(DisplayState {
      outputs: vec![DisplayOutput {
        name: "HDMI-1".to_string(),
        modes: vec![DisplayMode { width: 1920, height: 1080, refresh: 60 }],
        current: None,
      }],
      brightness: 80,
      night_light: true,
    })
    .unwrap();
    assert_eq!(
      value,
      json!({"outputs": [{"name": "HDMI-1", "modes": [{"width": 1920, "height": 1080, "refresh": 60}], "current": null}], "brightness": 80, "night_light": true})
    );
  }
}
