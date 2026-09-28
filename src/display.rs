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

use crate::json::JsonValue;
use crate::store::SettingsStore;

/// Store domain holding brightness, night light and per-output modes.
pub const DOMAIN: &str = "display";
pub const KEY_BRIGHTNESS: &str = "brightness";
pub const KEY_NIGHT_LIGHT: &str = "night_light";

pub const OP_DISPLAY_GET: &str = "display_get";
pub const OP_DISPLAY_SET: &str = "display_set";

pub const DEFAULT_BRIGHTNESS: u32 = 100;

/// One output mode: resolution plus refresh rate in Hz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayMode {
  pub width: i32,
  pub height: i32,
  pub refresh: u32,
}

impl DisplayMode {
  pub fn to_json_value(&self) -> JsonValue {
    JsonValue::Object(vec![
      ("width".to_string(), JsonValue::Integer(self.width as i64)),
      ("height".to_string(), JsonValue::Integer(self.height as i64)),
      ("refresh".to_string(), JsonValue::Integer(self.refresh as i64)),
    ])
  }

  pub fn from_json_value(value: &JsonValue) -> Option<Self> {
    Some(Self {
      width: value.get("width")?.as_i64()? as i32,
      height: value.get("height")?.as_i64()? as i32,
      refresh: value.get("refresh")?.as_u64()? as u32,
    })
  }
}

/// One output with its modes and live current mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayOutput {
  pub name: String,
  pub modes: Vec<DisplayMode>,
  pub current: Option<DisplayMode>,
}

impl DisplayOutput {
  pub fn to_json_value(&self) -> JsonValue {
    JsonValue::Object(vec![
      ("name".to_string(), JsonValue::Str(self.name.clone())),
      (
        "modes".to_string(),
        JsonValue::Array(self.modes.iter().map(|m| m.to_json_value()).collect()),
      ),
      (
        "current".to_string(),
        match &self.current {
          Some(mode) => mode.to_json_value(),
          None => JsonValue::Null,
        },
      ),
    ])
  }

  pub fn from_json_value(value: &JsonValue) -> Option<Self> {
    let modes = value
      .get("modes")?
      .as_array()?
      .iter()
      .filter_map(DisplayMode::from_json_value)
      .collect();
    let current = value
      .get("current")
      .and_then(DisplayMode::from_json_value);
    Some(Self {
      name: value.get("name")?.as_str()?.to_string(),
      modes,
      current,
    })
  }
}

/// Effective display state: live outputs plus stored settings.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayState {
  pub outputs: Vec<DisplayOutput>,
  pub brightness: u32,
  pub night_light: bool,
}

impl DisplayState {
  /// JSON shape for `display_get` / `display_set` replies.
  pub fn to_json_value(&self) -> JsonValue {
    JsonValue::Object(vec![
      (
        "outputs".to_string(),
        JsonValue::Array(self.outputs.iter().map(|o| o.to_json_value()).collect()),
      ),
      ("brightness".to_string(), JsonValue::Integer(self.brightness as i64)),
      ("night_light".to_string(), JsonValue::Bool(self.night_light)),
    ])
  }
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
  let reply = compositor_call(JsonValue::Object(vec![(
    "op".to_string(),
    JsonValue::Str("get_displays".to_string()),
  )]))?;
  let outputs = reply
    .get("outputs")
    .and_then(|v| v.as_array())
    .cloned()
    .unwrap_or_default();
  let mut out = Vec::new();
  for item in &outputs {
    let output = DisplayOutput::from_json_value(item)
      .ok_or_else(|| "display get failed: outputs invalid".to_string())?;
    out.push(output);
  }
  Ok(out)
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
fn compositor_call(request: JsonValue) -> Result<JsonValue, String> {
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
  let mut line = request.stringify(false);
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
  let frame = JsonValue::parse(&reply)
    .map_err(|e| format!("display failed: compositor reply invalid: {}", e))?;
  if frame.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
    Ok(frame.get("result").cloned().unwrap_or(JsonValue::Null))
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
  let mut flat: Vec<(String, JsonValue)> = vec![("op".to_string(), JsonValue::Str("set_display".to_string()))];
  if let Some(output) = output {
    if output.is_empty() {
      return Err("display set failed: output must not be empty".to_string());
    }
    flat.push(("output".to_string(), JsonValue::Str(output.to_string())));
  }
  if let Some(width) = width {
    if width <= 0 {
      return Err(format!("display set failed: invalid width: {}", width));
    }
    flat.push(("width".to_string(), JsonValue::Integer(width)));
  }
  if let Some(height) = height {
    if height <= 0 {
      return Err(format!("display set failed: invalid height: {}", height));
    }
    flat.push(("height".to_string(), JsonValue::Integer(height)));
  }
  if let Some(refresh) = refresh {
    flat.push(("refresh".to_string(), JsonValue::Integer(refresh as i64)));
  }
  if let Some(brightness) = brightness {
    flat.push(("brightness".to_string(), JsonValue::Float(brightness)));
  }
  if let Some(night_light) = night_light {
    flat.push(("night_light".to_string(), JsonValue::Bool(night_light)));
  }
  let result = compositor_call(JsonValue::Object(flat))?;
  let mut guard = store
    .lock()
    .map_err(|_| "display set failed: store is locked".to_string())?;
  if let Some(value) = brightness {
    guard.set(DOMAIN, KEY_BRIGHTNESS, JsonValue::Integer(value.round() as i64));
  }
  if let Some(night_light) = night_light {
    guard.set(DOMAIN, KEY_NIGHT_LIGHT, JsonValue::Bool(night_light));
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
  let flat = JsonValue::Object(vec![
    ("op".to_string(), JsonValue::Str("set_display".to_string())),
    ("brightness".to_string(), JsonValue::Float(brightness_in(store) as f64)),
    ("night_light".to_string(), JsonValue::Bool(night_light_in(store))),
  ]);
  compositor_call(flat)?;
  for name in store.keys(DOMAIN) {
    if name == KEY_BRIGHTNESS || name == KEY_NIGHT_LIGHT {
      continue;
    }
    if let Some(mode) = stored_mode(store, &name) {
      let flat = JsonValue::Object(vec![
        ("op".to_string(), JsonValue::Str("set_display".to_string())),
        ("output".to_string(), JsonValue::Str(name.clone())),
        ("width".to_string(), JsonValue::Integer(mode.width as i64)),
        ("height".to_string(), JsonValue::Integer(mode.height as i64)),
        ("refresh".to_string(), JsonValue::Integer(mode.refresh as i64)),
      ]);
      compositor_call(flat)?;
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
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
    store.set(DOMAIN, KEY_BRIGHTNESS, JsonValue::Integer(80));
    store.set(DOMAIN, KEY_NIGHT_LIGHT, JsonValue::Bool(true));
    store.set(
      DOMAIN,
      "HDMI-1",
      JsonValue::Object(vec![
        ("width".to_string(), JsonValue::Integer(1920)),
        ("height".to_string(), JsonValue::Integer(1080)),
        ("refresh".to_string(), JsonValue::Integer(120)),
      ]),
    );
    assert_eq!(brightness_in(&store), 80);
    assert!(night_light_in(&store));
    assert_eq!(
      stored_mode(&store, "HDMI-1"),
      Some(DisplayMode { width: 1920, height: 1080, refresh: 120 })
    );
    // Out-of-range brightness clamps, unknown shapes miss.
    store.set(DOMAIN, KEY_BRIGHTNESS, JsonValue::Integer(250));
    assert_eq!(brightness_in(&store), 100);
    store.set(
      DOMAIN,
      "DP-1",
      JsonValue::Object(vec![("width".to_string(), JsonValue::Integer(1920))]),
    );
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
    let value = DisplayState {
      outputs: vec![DisplayOutput {
        name: "HDMI-1".to_string(),
        modes: vec![DisplayMode { width: 1920, height: 1080, refresh: 60 }],
        current: None,
      }],
      brightness: 80,
      night_light: true,
    }
    .to_json_value();
    let expected = JsonValue::Object(vec![
      (
        "outputs".to_string(),
        JsonValue::Array(vec![JsonValue::Object(vec![
          ("name".to_string(), JsonValue::Str("HDMI-1".to_string())),
          (
            "modes".to_string(),
            JsonValue::Array(vec![JsonValue::Object(vec![
              ("width".to_string(), JsonValue::Integer(1920)),
              ("height".to_string(), JsonValue::Integer(1080)),
              ("refresh".to_string(), JsonValue::Integer(60)),
            ])]),
          ),
          ("current".to_string(), JsonValue::Null),
        ])]),
      ),
      ("brightness".to_string(), JsonValue::Integer(80)),
      ("night_light".to_string(), JsonValue::Bool(true)),
    ]);
    assert_eq!(value, expected);
  }
}
