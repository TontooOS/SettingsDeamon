use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::DaemonConfig;
use crate::library::LibraryManager;
use crate::store::SettingsStore;

// ---------------------------------------------------------------------------
// Read protocol (newline-delimited JSON, one request per line)
// ---------------------------------------------------------------------------
//
// Request:  {"id": 1, "op": "ping" | "get_hardware" | "get_os"
//                 | "wifi_list" | "wifi_status" | "wifi_known_list"
//                 | "wifi_connect" | "wifi_disconnect"
//                 | "wifi_enable" | "wifi_disable" | "wifi_forget"
//                 | "dns_get" | "dns_set"
//                 | "wired_list"
//                 | "customize_get" | "customize_set"
//                 | "wallpaper_get" | "wallpaper_set_current"
//                 | "wallpaper_set_fill" | "wallpaper_add"
//                 | "wallpaper_apply" | "wallpaper_delete"
//                 | "display_get" | "display_set",
//            "params": {...}}
// Success:  {"id": 1, "ok": true, "result": {...}}
// Failure:  {"id": 1, "ok": false, "error": "..."}
//
// `get_hardware` returns the parsed `sys.fico` content, `get_os` the parsed
// `os.fico` content. Missing or corrupt files return `ok: false`, never a
// partial document.
//
// `wifi_list`, `wifi_status` and `wifi_known_list` are public read ops
// served to every client (`wifi_known_list` never exposes passwords).
// `dns_get` is a public read op for the effective DNS state; `dns_set`
// is a private write op with the same visibility rule as the `wifi_*`
// write ops below. `wired_list` is a public read op for connected
// Ethernet interfaces with details.
// `wifi_connect`, `wifi_disconnect`, `wifi_enable`, `wifi_disable` and
// `wifi_forget` are private write ops: no public client library exposes
// them, only the Settings app (`com.tontoo.systemsettings`) may call them.
//
// `customize_get` is a public read op returning the effective
// customization (`{"wallpaper", "accent", "theme"}`).
// `customize_set` is a private write op with the same visibility rule as
// the `wifi_*` write ops: partial `{"wallpaper"?, "accent"?, "theme"?}`
// params, validated before anything is persisted.
//
// `wallpaper_get` is a public read op returning the full wallpaper state
// (`current`, `fill`, `customs`, `premade`).
// `wallpaper_set_current`, `wallpaper_set_fill` and `wallpaper_add` are
// private write ops with the same visibility rule: selection persistence
// only, nothing here applies the wallpaper to the desktop.
// `wallpaper_apply` resolves the variant file and forwards it to the
// compositor for the desktop crossfade, then persists the selection.

pub const OP_PING: &str = "ping";
pub const OP_GET_HARDWARE: &str = "get_hardware";
pub const OP_GET_OS: &str = "get_os";

/// Socket server with the read protocol. One thread per connection shares
/// the store so `customize_set` and wallpaper selection writes are visible
/// to every client.
#[cfg(target_os = "linux")]
pub fn run_server(
  config: &DaemonConfig,
  store: Arc<Mutex<SettingsStore>>,
  _libs: Arc<Mutex<LibraryManager>>,
) -> io::Result<()> {
  use std::os::unix::net::UnixListener;

  let _ = std::fs::remove_file(&config.socket_path);
  let listener = UnixListener::bind(&config.socket_path)?;
  log::info!("listening on {:?}", config.socket_path);

  for stream in listener.incoming() {
    match stream {
      Ok(stream) => {
        log::info!("connection from {:?}", stream.peer_addr().ok());
        let sys = config.sys_fico_path.clone();
        let os = config.os_fico_path.clone();
        let store = store.clone();
        std::thread::spawn(move || serve_connection(stream, sys, os, store));
      }
      Err(e) => log::warn!("accept failed: {}", e),
    }
  }
  Ok(())
}

/// Non-Linux stub so `cargo check` passes on macOS/Windows hosts.
/// Real socket support is Linux-only (TontooOS target).
#[cfg(not(target_os = "linux"))]
pub fn run_server(
  _config: &DaemonConfig,
  _store: Arc<Mutex<SettingsStore>>,
  _libs: Arc<Mutex<LibraryManager>>,
) -> io::Result<()> {
  Err(io::Error::new(
    io::ErrorKind::Unsupported,
    "settings-daemon socket server is only supported on Linux",
  ))
}

/// Serve one connection until EOF or a fatal I/O error. Malformed lines get
/// an error frame, the connection stays open.
#[cfg(target_os = "linux")]
fn serve_connection(
  stream: std::os::unix::net::UnixStream,
  sys: PathBuf,
  os: PathBuf,
  store: Arc<Mutex<SettingsStore>>,
) {
  use std::io::{BufRead, BufReader, Write};

  let mut reader = match stream.try_clone() {
    Ok(clone) => BufReader::new(clone),
    Err(_) => return,
  };
  let mut writer = stream;
  let mut line = String::new();
  loop {
    line.clear();
    match reader.read_line(&mut line) {
      Ok(0) => break, // EOF
      Ok(_) => {}
      Err(_) => break,
    }
    let frame = handle_line(&line, &sys, &os, &store);
    let mut out = frame.to_string();
    out.push('\n');
    if writer.write_all(out.as_bytes()).is_err() || writer.flush().is_err() {
      break;
    }
  }
}

/// Parse one request line and dispatch it. Never panics, always returns a
/// frame with the request `id` (0 when the line has no usable id).
fn handle_line(
  line: &str,
  sys: &Path,
  os: &Path,
  store: &Arc<Mutex<SettingsStore>>,
) -> serde_json::Value {
  let request: serde_json::Value = match serde_json::from_str(line) {
    Ok(value) => value,
    Err(e) => return error_frame(0, format!("invalid request: {}", e)),
  };
  let id = request.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
  let op = request.get("op").and_then(|v| v.as_str()).unwrap_or("");
  let params = request.get("params").cloned().unwrap_or(serde_json::Value::Null);
  dispatch(id, op, &params, sys, os, store)
}

fn dispatch(
  id: u64,
  op: &str,
  params: &serde_json::Value,
  sys: &Path,
  os: &Path,
  store: &Arc<Mutex<SettingsStore>>,
) -> serde_json::Value {
  match op {
    OP_PING => success_frame(id, serde_json::json!({"pong": true})),
    OP_GET_HARDWARE => match read_fico_json(sys) {
      Ok(json) => success_frame(id, json),
      Err(e) => error_frame(id, format!("sys.fico unavailable: {}", e)),
    },
    OP_GET_OS => match read_fico_json(os) {
      Ok(json) => success_frame(id, json),
      Err(e) => error_frame(id, format!("os.fico unavailable: {}", e)),
    },
    crate::wifi::OP_WIFI_LIST => match crate::wifi::list() {
      Ok(networks) => success_frame(id, serde_json::json!({"networks": networks})),
      Err(e) => error_frame(id, format!("wifi scan failed: {}", e)),
    },
    crate::wifi::OP_WIFI_STATUS => match crate::wifi::status() {
      Ok(value) => success_frame(id, value),
      Err(e) => error_frame(id, format!("wifi status failed: {}", e)),
    },
    crate::wifi::OP_WIFI_KNOWN_LIST => match crate::wifi::known_list() {
      Ok(known) => success_frame(id, serde_json::json!({"networks": known})),
      Err(e) => error_frame(id, format!("wifi known list failed: {}", e)),
    },
    crate::wifi::OP_WIFI_CONNECT => {
      let ssid = params.get("ssid").and_then(|v| v.as_str()).unwrap_or("");
      let password = params.get("password").and_then(|v| v.as_str());
      let hidden = params.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false);
      match crate::wifi::connect(ssid, password, hidden) {
        Ok(status) => success_frame(
          id,
          serde_json::to_value(status).unwrap_or(serde_json::Value::Null),
        ),
        Err(e) => error_frame(id, format!("wifi connect failed: {}", e)),
      }
    }
    crate::wifi::OP_WIFI_DISCONNECT => match crate::wifi::disconnect() {
      Ok(()) => success_frame(id, serde_json::json!({"disconnected": true})),
      Err(e) => error_frame(id, format!("wifi disconnect failed: {}", e)),
    },
    crate::wifi::OP_WIFI_ENABLE => match crate::wifi::set_enabled(true) {
      Ok(()) => success_frame(id, serde_json::json!({"enabled": true})),
      Err(e) => error_frame(id, format!("wifi enable failed: {}", e)),
    },
    crate::wifi::OP_WIFI_DISABLE => match crate::wifi::set_enabled(false) {
      Ok(()) => success_frame(id, serde_json::json!({"enabled": false})),
      Err(e) => error_frame(id, format!("wifi disable failed: {}", e)),
    },
    crate::wifi::OP_WIFI_FORGET => {
      let ssid = params.get("ssid").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wifi::forget(ssid) {
        Ok(removed) => success_frame(id, serde_json::json!({"forgotten": removed})),
        Err(e) => error_frame(id, format!("wifi forget failed: {}", e)),
      }
    }
    crate::dns::OP_DNS_GET => match crate::dns::get() {
      Ok(state) => success_frame(
        id,
        serde_json::to_value(state).unwrap_or(serde_json::Value::Null),
      ),
      Err(e) => error_frame(id, format!("dns get failed: {}", e)),
    },
    crate::dns::OP_DNS_SET => {
      let servers = params.get("servers").and_then(|v| v.as_str()).unwrap_or("");
      match crate::dns::set_from_str(servers) {
        Ok(state) => success_frame(
          id,
          serde_json::to_value(state).unwrap_or(serde_json::Value::Null),
        ),
        Err(e) => error_frame(id, format!("dns set failed: {}", e)),
      }
    }
    crate::wired::OP_WIRED_LIST => match crate::wired::list() {
      Ok(interfaces) => success_frame(id, serde_json::json!({"interfaces": interfaces})),
      Err(e) => error_frame(id, format!("wired list failed: {}", e)),
    }
    crate::customize::OP_CUSTOMIZE_GET => match store.lock() {
      Ok(guard) => success_frame(
        id,
        serde_json::to_value(crate::customize::get(&guard))
          .unwrap_or(serde_json::Value::Null),
      ),
      Err(_) => error_frame(id, "customize get failed: store is locked".to_string()),
    },
    crate::customize::OP_CUSTOMIZE_SET => {
      let wallpaper = params.get("wallpaper").and_then(|v| v.as_str());
      let accent = params.get("accent").and_then(|v| v.as_str());
      let theme = params.get("theme").and_then(|v| v.as_str());
      match crate::customize::set(store, wallpaper, accent, theme) {
        Ok(settings) => success_frame(
          id,
          serde_json::to_value(settings).unwrap_or(serde_json::Value::Null),
        ),
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_GET => match store.lock() {
      Ok(guard) => success_frame(
        id,
        serde_json::to_value(crate::wallpaper::state(&guard))
          .unwrap_or(serde_json::Value::Null),
      ),
      Err(_) => error_frame(id, "wallpaper get failed: store is locked".to_string()),
    },
    crate::wallpaper::OP_WALLPAPER_SET_CURRENT => {
      let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("");
      let entry_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wallpaper::set_current(store, kind, entry_id) {
        Ok(entry) => success_frame(
          id,
          serde_json::to_value(entry).unwrap_or(serde_json::Value::Null),
        ),
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_SET_FILL => {
      let fill = params.get("fill").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wallpaper::set_fill(store, fill) {
        Ok(applied) => success_frame(id, serde_json::json!({"fill": applied})),
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_ADD => {
      let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
      let name = params.get("name").and_then(|v| v.as_str());
      match crate::wallpaper::add(Path::new(path), name) {
        Ok(entry) => success_frame(
          id,
          serde_json::to_value(entry).unwrap_or(serde_json::Value::Null),
        ),
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_APPLY => {
      let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("");
      let entry_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
      let variant = params.get("variant").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wallpaper::apply(store, kind, entry_id, variant) {
        Ok(entry) => success_frame(
          id,
          serde_json::to_value(entry).unwrap_or(serde_json::Value::Null),
        ),
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_DELETE => {
      let entry_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wallpaper::delete(store, entry_id) {
        Ok(switched) => success_frame(id, serde_json::json!({"deleted": true, "switched": switched})),
        Err(e) => error_frame(id, e),
      }
    }
    crate::display::OP_DISPLAY_GET => match store
      .lock()
      .map_err(|_| "display get failed: store is locked".to_string())
      .and_then(|guard| crate::display::get(&guard))
    {
      Ok(state) => success_frame(
        id,
        serde_json::to_value(state).unwrap_or(serde_json::Value::Null),
      ),
      Err(e) => error_frame(id, e),
    },
    crate::display::OP_DISPLAY_SET => {
      let output = params.get("output").and_then(|v| v.as_str());
      let width = params.get("width").and_then(|v| v.as_i64());
      let height = params.get("height").and_then(|v| v.as_i64());
      let refresh = params.get("refresh").and_then(|v| v.as_u64()).map(|v| v as u32);
      let brightness = params.get("brightness").and_then(|v| v.as_f64());
      let night_light = params.get("night_light").and_then(|v| v.as_bool());
      match crate::display::set(store, output, width, height, refresh, brightness, night_light) {
        Ok(state) => success_frame(
          id,
          serde_json::to_value(state).unwrap_or(serde_json::Value::Null),
        ),
        Err(e) => error_frame(id, e),
      }
    }
    _ => error_frame(id, format!("unknown op: {:?}", op)),
  }
}

/// Read a `.fico` file from disk and return its content as JSON.
fn read_fico_json(path: &Path) -> Result<serde_json::Value, String> {
  let doc = sdk::FishFile::FishDocument::from_file(path).map_err(|e| e.to_string())?;
  Ok(doc.to_json_value())
}

fn success_frame(id: u64, result: serde_json::Value) -> serde_json::Value {
  serde_json::json!({"id": id, "ok": true, "result": result})
}

fn error_frame(id: u64, error: String) -> serde_json::Value {
  serde_json::json!({"id": id, "ok": false, "error": error})
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  fn no_params() -> serde_json::Value {
    serde_json::Value::Null
  }

  fn temp_case(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tontoo-socket-{}", name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
  }

  fn memory_store() -> Arc<Mutex<SettingsStore>> {
    Arc::new(Mutex::new(SettingsStore::new(PathBuf::from(
      "/tmp/tontoo-settings-socket-test.json",
    ))))
  }

  #[test]
  fn ping_frame() {
    let store = memory_store();
    let frame = dispatch(7, OP_PING, &no_params(), Path::new("/nonexistent"), Path::new("/nonexistent"), &store);
    assert_eq!(frame["id"], 7);
    assert_eq!(frame["ok"], true);
    assert_eq!(frame["result"]["pong"], true);
  }

  #[test]
  fn unknown_op_frame() {
    let store = memory_store();
    let frame = dispatch(3, "delete_everything", &no_params(), Path::new("/nonexistent"), Path::new("/nonexistent"), &store);
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("unknown op"));
  }

  #[test]
  fn invalid_line_frame() {
    let store = memory_store();
    let frame = handle_line("not json\n", Path::new("/nonexistent"), Path::new("/nonexistent"), &store);
    assert_eq!(frame["id"], 0);
    assert_eq!(frame["ok"], false);
  }

  #[test]
  fn missing_file_frame() {
    let store = memory_store();
    let frame = dispatch(1, OP_GET_OS, &no_params(), Path::new("/nonexistent"), Path::new("/nonexistent-sys.fico"), &store);
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("os.fico unavailable"));
  }

  #[test]
  fn wifi_connect_rejects_missing_ssid() {
    let store = memory_store();
    let frame = dispatch(
      4,
      crate::wifi::OP_WIFI_CONNECT,
      &serde_json::json!({}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("wifi connect failed"));
  }

  #[test]
  fn wifi_forget_rejects_missing_ssid() {
    let store = memory_store();
    let frame = dispatch(
      5,
      crate::wifi::OP_WIFI_FORGET,
      &serde_json::json!({}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("wifi forget failed"));
  }

  #[test]
  fn dns_set_rejects_invalid_servers() {
    let store = memory_store();
    let frame = dispatch(
      7,
      crate::dns::OP_DNS_SET,
      &serde_json::json!({"servers": "not-an-ip"}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("dns set failed"));
  }

  #[test]
  fn customize_get_returns_defaults() {
    let store = memory_store();
    let frame = dispatch(
      6,
      crate::customize::OP_CUSTOMIZE_GET,
      &no_params(),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], true);
    assert_eq!(frame["result"]["wallpaper"], crate::customize::DEFAULT_WALLPAPER);
    assert_eq!(frame["result"]["accent"], crate::customize::DEFAULT_ACCENT);
    assert_eq!(frame["result"]["theme"], crate::customize::DEFAULT_THEME);
  }

  #[test]
  fn customize_set_roundtrip_and_rejects_invalid() {
    let store = memory_store();
    let frame = dispatch(
      7,
      crate::customize::OP_CUSTOMIZE_SET,
      &serde_json::json!({"wallpaper": "SONOMA", "accent": "blue", "theme": "light"}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], true);
    assert_eq!(frame["result"]["wallpaper"], "SONOMA");

    let frame = dispatch(
      8,
      crate::customize::OP_CUSTOMIZE_GET,
      &no_params(),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["result"]["accent"], "blue");

    let frame = dispatch(
      9,
      crate::customize::OP_CUSTOMIZE_SET,
      &serde_json::json!({"theme": "sepia"}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("unknown theme"));
    let _ = std::fs::remove_file("/tmp/tontoo-settings-socket-test.json");
  }

  #[test]
  fn wallpaper_get_returns_state_shape() {
    let store = memory_store();
    let frame = dispatch(
      10,
      crate::wallpaper::OP_WALLPAPER_GET,
      &no_params(),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], true);
    assert!(frame["result"]["premade"].is_array());
    assert!(frame["result"]["customs"].is_array());
    assert_eq!(frame["result"]["fill"], crate::wallpaper::DEFAULT_FILL);
  }

  #[test]
  fn wallpaper_set_fill_roundtrip_and_rejects_unknown() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    // No THAOELAKE pack here: nothing configured, persist-only path.
    let premade = temp_case("fill-premade");
    std::fs::create_dir_all(premade.join("FLOW")).unwrap();
    std::fs::write(premade.join("FLOW").join("a.png"), b"x").unwrap();
    std::env::set_var("TONTOO_WALLPAPERS_DIR", &premade);
    let customs = temp_case("fill-custom");
    std::env::set_var("SETTINGS_WALLPAPER_DIR", &customs);
    let store = memory_store();
    let frame = dispatch(
      11,
      crate::wallpaper::OP_WALLPAPER_SET_FILL,
      &serde_json::json!({"fill": "tile"}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], true);
    assert_eq!(frame["result"]["fill"], "tile");
    let frame = dispatch(
      12,
      crate::wallpaper::OP_WALLPAPER_SET_FILL,
      &serde_json::json!({"fill": "melt"}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("unknown fill mode"));
    let _ = std::fs::remove_file("/tmp/tontoo-settings-socket-test.json");
    std::env::remove_var("TONTOO_WALLPAPERS_DIR");
    std::env::remove_var("SETTINGS_WALLPAPER_DIR");
    let _ = std::fs::remove_dir_all(&premade);
    let _ = std::fs::remove_dir_all(&customs);
  }

  #[test]
  fn wallpaper_set_current_rejects_unknown() {
    let store = memory_store();
    let frame = dispatch(
      13,
      crate::wallpaper::OP_WALLPAPER_SET_CURRENT,
      &serde_json::json!({"kind": "orb", "id": "FLOW"}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("unknown kind"));
  }

  #[test]
  fn wallpaper_add_rejects_missing_file() {
    let store = memory_store();
    let frame = dispatch(
      14,
      crate::wallpaper::OP_WALLPAPER_ADD,
      &serde_json::json!({"path": "/nonexistent-wallpaper-test/missing.png"}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("file not found"));
  }

  #[test]
  fn wallpaper_apply_rejects_unknown_variant() {
    let store = memory_store();
    let frame = dispatch(
      15,
      crate::wallpaper::OP_WALLPAPER_APPLY,
      &serde_json::json!({"kind": "premade", "id": "FLOW", "variant": "sepia"}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("unknown variant"));
  }

  #[test]
  fn wallpaper_delete_rejects_unknown_id() {
    let store = memory_store();
    let frame = dispatch(
      16,
      crate::wallpaper::OP_WALLPAPER_DELETE,
      &serde_json::json!({"id": "ghost"}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("unknown custom wallpaper"));
  }

  #[test]
  fn display_set_validates_before_touching_anything() {
    let store = memory_store();
    let frame = dispatch(
      17,
      crate::display::OP_DISPLAY_SET,
      &serde_json::json!({"brightness": 120.0}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("invalid brightness"));
  }

  #[test]
  fn display_get_needs_the_compositor() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    std::env::set_var(
      "COMPOSITOR_SOCKET",
      "/nonexistent-display-test/compositor.sock",
    );
    let store = memory_store();
    let frame = dispatch(
      18,
      crate::display::OP_DISPLAY_GET,
      &no_params(),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("unreachable"));
    std::env::remove_var("COMPOSITOR_SOCKET");
  }

  /// Mock compositor socket replying canned frames in order, one per
  /// connection.
  fn mock_display(replies: Vec<serde_json::Value>) -> PathBuf {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
      "tontoo-display-mock-{}-{}.sock",
      std::process::id(),
      NEXT_ID.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    std::thread::spawn(move || {
      for reply in replies {
        let Ok((mut stream, _)) = listener.accept() else {
          break;
        };
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
  fn display_set_roundtrip_persists() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let sock = mock_display(vec![
      serde_json::json!({"ok": true, "result": {
        "output": "HDMI-1",
        "mode": {"width": 1920, "height": 1080, "refresh": 120},
        "brightness": 80,
        "night_light": true,
      }}),
      serde_json::json!({"ok": true, "result": {"outputs": []}}),
    ]);
    std::env::set_var("COMPOSITOR_SOCKET", &sock);
    let store = memory_store();
    let frame = dispatch(
      19,
      crate::display::OP_DISPLAY_SET,
      &serde_json::json!({"brightness": 80.0, "night_light": true}),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
    );
    assert_eq!(frame["ok"], true);
    assert_eq!(frame["result"]["brightness"], 80);
    assert_eq!(frame["result"]["night_light"], true);
    assert_eq!(
      frame["result"]["outputs"],
      serde_json::json!([])
    );
    std::env::remove_var("COMPOSITOR_SOCKET");
    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_file("/tmp/tontoo-settings-socket-test.json");
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn connection_roundtrip() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let dir = std::env::temp_dir().join("settings-daemon-socket-test");
    let _ = std::fs::create_dir_all(&dir);
    let os_path = dir.join("os.fico");
    std::fs::write(&os_path, "os {\n    version: \"26.1.0\"\n    beta: false\n}\n").unwrap();
    let sys_path = dir.join("sys.fico");
    std::fs::write(&sys_path, "ram {\n    total_gb: 32.0\n}\n").unwrap();

    let (client, server) = UnixStream::pair().unwrap();
    let sys_clone = sys_path.clone();
    let os_clone = os_path.clone();
    let store = Arc::new(Mutex::new(SettingsStore::new(
      dir.join("socket-test-settings.json"),
    )));
    std::thread::spawn(move || serve_connection(server, sys_clone, os_clone, store));

    let mut writer = client.try_clone().unwrap();
    let mut reader = BufReader::new(client);
    let mut line = String::new();

    writer.write_all(b"{\"id\": 1, \"op\": \"get_os\"}\n").unwrap();
    writer.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["ok"], true);
    assert_eq!(frame["result"]["os"]["version"], "26.1.0");

    line.clear();
    writer.write_all(b"{\"id\": 2, \"op\": \"get_hardware\"}\n").unwrap();
    writer.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["result"]["ram"]["total_gb"], 32.0);

    let _ = std::fs::remove_file(&os_path);
    let _ = std::fs::remove_file(&sys_path);
  }
}
