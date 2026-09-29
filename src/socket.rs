use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use crate::config::DaemonConfig;
use crate::json::JsonValue;
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
//                 | "datetime_get" | "datetime_set_timezone"
//                 | "datetime_set_24h"
//                 | "locale_get" | "locale_set_language" | "locale_set_region"
//                 | "locale_set_keymap" | "locale_set_auto_keymap"
//                 | "locale_keymap_variants"
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
// Ethernet interfaces with details. `datetime_get` is a public read op
// for NTP/timezone/24h state; `datetime_set_timezone` and
// `datetime_set_24h` are private write ops with the same visibility rule
// as the `wifi_*` write ops. `locale_get` and `locale_keymap_variants`
// are public read ops; the `locale_set_*` ops are private write ops with
// the same visibility rule.
// `wifi_connect`, `wifi_disconnect`, `wifi_enable`, `wifi_disable` and
// `wifi_forget` are private write ops: no public client library exposes
// them, only the Settings app (`com.tontoo.systemsettings`) may call them.
//
// `customize_get` is a public read op returning the effective
// customization (`{"wallpaper", "accent", "theme", "revision"}`).
// `customize_set` is a private write op with the same visibility rule as
// the `wifi_*` write ops: partial
// `{"wallpaper"?, "accent"?, "theme"?, "glass"?}` params, validated before
// anything is persisted. Every successful set bumps `revision` so clients
// can poll for changes.
//
// `wallpaper_get` is a public read op returning the full wallpaper state
// (`current`, `fill`, `customs`, `premade`).
// `wallpaper_set_current`, `wallpaper_set_fill` and `wallpaper_add` are
// private write ops with the same visibility rule: selection persistence
// only, nothing here applies the wallpaper to the desktop.
// `wallpaper_apply` resolves the variant file and forwards it to the
// compositor for the desktop crossfade, then persists the selection.
//
// `subscribe` registers the connection for change events: after every
// successful write op the daemon pushes
// `{"event": "<domain>_changed", "result": {...}}` frames on connections
// subscribed to that event (empty filter means all events). Subscribed
// clients need no polling; `revision` in `customize_changed` payloads
// keeps poll-based clients in sync.

pub const OP_PING: &str = "ping";
pub const OP_GET_HARDWARE: &str = "get_hardware";
pub const OP_GET_OS: &str = "get_os";
pub const OP_SUBSCRIBE: &str = "subscribe";

/// Event pushed after a successful WiFi write op.
pub const EVENT_WIFI_CHANGED: &str = "wifi_changed";
/// Event pushed after a successful `dns_set`.
pub const EVENT_DNS_CHANGED: &str = "dns_changed";
/// Event pushed after a successful `datetime_set_*`.
pub const EVENT_DATETIME_CHANGED: &str = "datetime_changed";
/// Event pushed after a successful `locale_set_*`.
pub const EVENT_LOCALE_CHANGED: &str = "locale_changed";
/// Event pushed after a successful `customize_set` (carries `revision`).
pub const EVENT_CUSTOMIZE_CHANGED: &str = "customize_changed";
/// Event pushed after a successful `wallpaper_*` write op.
pub const EVENT_WALLPAPER_CHANGED: &str = "wallpaper_changed";
/// Event pushed after a successful `display_set`.
pub const EVENT_DISPLAY_CHANGED: &str = "display_changed";

#[derive(Debug)]
struct Subscriber {
  id: u64,
  sender: mpsc::Sender<String>,
  filter: Vec<String>,
}

/// Fan-out for daemon change events. Connections register with `subscribe`
/// and receive `{"event": name, "result": {...}}` frames whenever a write
/// op they listen for succeeds, so clients need no polling.
#[derive(Debug, Default)]
pub struct Broadcaster {
  next_id: Mutex<u64>,
  subscribers: Mutex<Vec<Subscriber>>,
}

impl Broadcaster {
  pub fn new() -> Self {
    Self {
      next_id: Mutex::new(1),
      subscribers: Mutex::new(Vec::new()),
    }
  }

  /// Shared handle for the server and its connection threads.
  pub fn shared() -> Arc<Self> {
    Arc::new(Self::new())
  }

  /// Register a subscriber. An empty filter receives every event.
  /// Returns the subscription id plus the receiving end.
  fn register(&self, filter: Vec<String>) -> (u64, mpsc::Receiver<String>) {
    let (sender, receiver) = mpsc::channel();
    let mut next = self.next_id.lock().unwrap();
    let id = *next;
    *next += 1;
    drop(next);
    self.subscribers.lock().unwrap().push(Subscriber { id, sender, filter });
    (id, receiver)
  }

  /// Drop a subscription. Missing ids are ignored.
  fn unregister(&self, id: u64) {
    self.subscribers.lock().unwrap().retain(|s| s.id != id);
  }

  /// Push an event to every matching subscriber. Dead receivers are
  /// dropped. Never blocks, never fails.
  pub fn publish(&self, event: &str, result: &JsonValue) {
    let mut line = JsonValue::Object(vec![
      ("event".to_string(), JsonValue::Str(event.to_string())),
      ("result".to_string(), result.clone()),
    ])
    .stringify(false);
    line.push('\n');
    self.subscribers.lock().unwrap().retain(|subscriber| {
      if subscriber.filter.is_empty()
        || subscriber.filter.iter().any(|name| name == event)
      {
        subscriber.sender.send(line.clone()).is_ok()
      } else {
        true
      }
    });
  }

  #[cfg(test)]
  fn subscriber_count(&self) -> usize {
    self.subscribers.lock().unwrap().len()
  }
}

/// Socket server with the read protocol. One thread per connection shares
/// the store so `customize_set` and wallpaper selection writes are visible
/// to every client. `broadcaster` fans write-op events out to subscribers.
#[cfg(target_os = "linux")]
pub fn run_server(
  config: &DaemonConfig,
  store: Arc<Mutex<SettingsStore>>,
  _libs: Arc<Mutex<LibraryManager>>,
  broadcaster: Arc<Broadcaster>,
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
        let broadcaster = broadcaster.clone();
        std::thread::spawn(move || serve_connection(stream, sys, os, store, broadcaster));
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
  _broadcaster: Arc<Broadcaster>,
) -> io::Result<()> {
  Err(io::Error::new(
    io::ErrorKind::Unsupported,
    "settings-daemon socket server is only supported on Linux",
  ))
}

/// Serve one connection until EOF or a fatal I/O error. Malformed lines get
/// an error frame, the connection stays open. A successful `subscribe`
/// spawns a writer thread pushing event frames until the connection drops.
#[cfg(target_os = "linux")]
fn serve_connection(
  stream: std::os::unix::net::UnixStream,
  sys: PathBuf,
  os: PathBuf,
  store: Arc<Mutex<SettingsStore>>,
  broadcaster: Arc<Broadcaster>,
) {
  use std::io::{BufRead, BufReader, Write};

  let mut reader = match stream.try_clone() {
    Ok(clone) => BufReader::new(clone),
    Err(_) => return,
  };
  let writer = Arc::new(Mutex::new(stream));
  let mut subscription_id: Option<u64> = None;
  let mut line = String::new();
  loop {
    line.clear();
    match reader.read_line(&mut line) {
      Ok(0) => break, // EOF
      Ok(_) => {}
      Err(_) => break,
    }
    let mut pending_filter = None;
    let frame = handle_line(&line, &sys, &os, &store, &broadcaster, &mut pending_filter);
    if let Some(filter) = pending_filter.take() {
      if let Some(id) = subscription_id.take() {
        broadcaster.unregister(id);
      }
      let (id, events) = broadcaster.register(filter);
      subscription_id = Some(id);
      let writer_clone = writer.clone();
      let broadcaster_clone = broadcaster.clone();
      std::thread::spawn(move || {
        for message in events {
          let delivered = writer_clone
            .lock()
            .map(|mut stream| {
              stream.write_all(message.as_bytes()).and_then(|_| stream.flush()).is_ok()
            })
            .unwrap_or(false);
          if !delivered {
            break;
          }
        }
        broadcaster_clone.unregister(id);
      });
    }
    let mut out = frame.stringify(false);
    out.push('\n');
    let delivered = writer
      .lock()
      .map(|mut stream| {
        stream.write_all(out.as_bytes()).and_then(|_| stream.flush()).is_ok()
      })
      .unwrap_or(false);
    if !delivered {
      break;
    }
  }
  if let Some(id) = subscription_id {
    broadcaster.unregister(id);
  }
}

/// Parse one request line and dispatch it. Never panics, always returns a
/// frame with the request `id` (0 when the line has no usable id).
/// A successful `subscribe` additionally reports its event filter through
/// `out_subscribe` so the caller can attach the connection.
fn handle_line(
  line: &str,
  sys: &Path,
  os: &Path,
  store: &Arc<Mutex<SettingsStore>>,
  broadcaster: &Broadcaster,
  out_subscribe: &mut Option<Vec<String>>,
) -> JsonValue {
  let request = match JsonValue::parse(line) {
    Ok(value) => value,
    Err(e) => return error_frame(0, format!("invalid request: {}", e)),
  };
  let id = crate::json::frame_id(&request);
  let op = crate::json::frame_op(&request);
  let params = crate::json::frame_params(&request);
  dispatch(id, &op, &params, sys, os, store, broadcaster, out_subscribe)
}

fn dispatch(
  id: u64,
  op: &str,
  params: &JsonValue,
  sys: &Path,
  os: &Path,
  store: &Arc<Mutex<SettingsStore>>,
  broadcaster: &Broadcaster,
  out_subscribe: &mut Option<Vec<String>>,
) -> JsonValue {
  match op {
    OP_PING => success_frame(
      id,
      JsonValue::Object(vec![("pong".to_string(), JsonValue::Bool(true))]),
    ),
    OP_SUBSCRIBE => {
      let filter = match params.get("events") {
        None | Some(JsonValue::Null) => Vec::new(),
        Some(JsonValue::Array(items)) => {
          let mut names = Vec::with_capacity(items.len());
          for item in items {
            match item.as_str() {
              Some(name) => names.push(name.to_string()),
              None => {
                return error_frame(id, "subscribe failed: events must be strings".to_string());
              }
            }
          }
          names
        }
        Some(_) => {
          return error_frame(id, "subscribe failed: events must be an array".to_string());
        }
      };
      let reply_events =
        JsonValue::Array(filter.iter().map(|name| JsonValue::Str(name.clone())).collect());
      *out_subscribe = Some(filter);
      success_frame(
        id,
        JsonValue::Object(vec![
          ("subscribed".to_string(), JsonValue::Bool(true)),
          ("events".to_string(), reply_events),
        ]),
      )
    }
    OP_GET_HARDWARE => match read_fico_json(sys) {
      Ok(json) => success_frame(id, json),
      Err(e) => error_frame(id, format!("sys.fico unavailable: {}", e)),
    },
    OP_GET_OS => match read_fico_json(os) {
      Ok(json) => success_frame(id, json),
      Err(e) => error_frame(id, format!("os.fico unavailable: {}", e)),
    },
    crate::wifi::OP_WIFI_LIST => match crate::wifi::list() {
      Ok(networks) => success_frame(
        id,
        JsonValue::Object(vec![(
          "networks".to_string(),
          JsonValue::Array(networks.iter().map(crate::wifi::network_to_json).collect()),
        )]),
      ),
      Err(e) => error_frame(id, format!("wifi scan failed: {}", e)),
    },
    crate::wifi::OP_WIFI_STATUS => match crate::wifi::status() {
      Ok(value) => success_frame(id, value),
      Err(e) => error_frame(id, format!("wifi status failed: {}", e)),
    },
    crate::wifi::OP_WIFI_KNOWN_LIST => match crate::wifi::known_list() {
      Ok(known) => success_frame(
        id,
        JsonValue::Object(vec![(
          "networks".to_string(),
          JsonValue::Array(known.iter().map(|n| n.to_json_value()).collect()),
        )]),
      ),
      Err(e) => error_frame(id, format!("wifi known list failed: {}", e)),
    },
    crate::wifi::OP_WIFI_CONNECT => {
      let ssid = params.get("ssid").and_then(|v| v.as_str()).unwrap_or("");
      let password = params.get("password").and_then(|v| v.as_str());
      let hidden = params.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false);
      match crate::wifi::connect(ssid, password, hidden) {
        Ok(status) => {
          let payload = crate::wifi::status_to_json(&status);
          broadcaster.publish(EVENT_WIFI_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, format!("wifi connect failed: {}", e)),
      }
    }
    crate::wifi::OP_WIFI_DISCONNECT => match crate::wifi::disconnect() {
      Ok(()) => {
        let payload = JsonValue::Object(vec![("disconnected".to_string(), JsonValue::Bool(true))]);
        broadcaster.publish(EVENT_WIFI_CHANGED, &payload);
        success_frame(id, payload)
      }
      Err(e) => error_frame(id, format!("wifi disconnect failed: {}", e)),
    },
    crate::wifi::OP_WIFI_ENABLE => match crate::wifi::set_enabled(true) {
      Ok(()) => {
        let payload = JsonValue::Object(vec![("enabled".to_string(), JsonValue::Bool(true))]);
        broadcaster.publish(EVENT_WIFI_CHANGED, &payload);
        success_frame(id, payload)
      }
      Err(e) => error_frame(id, format!("wifi enable failed: {}", e)),
    },
    crate::wifi::OP_WIFI_DISABLE => match crate::wifi::set_enabled(false) {
      Ok(()) => {
        let payload = JsonValue::Object(vec![("enabled".to_string(), JsonValue::Bool(false))]);
        broadcaster.publish(EVENT_WIFI_CHANGED, &payload);
        success_frame(id, payload)
      }
      Err(e) => error_frame(id, format!("wifi disable failed: {}", e)),
    },
    crate::wifi::OP_WIFI_FORGET => {
      let ssid = params.get("ssid").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wifi::forget(ssid) {
        Ok(removed) => {
          let payload =
            JsonValue::Object(vec![("forgotten".to_string(), JsonValue::Bool(removed))]);
          broadcaster.publish(EVENT_WIFI_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, format!("wifi forget failed: {}", e)),
      }
    }
    crate::dns::OP_DNS_GET => match crate::dns::get() {
      Ok(state) => success_frame(id, state.to_json_value()),
      Err(e) => error_frame(id, format!("dns get failed: {}", e)),
    },
    crate::dns::OP_DNS_SET => {
      let servers = params.get("servers").and_then(|v| v.as_str()).unwrap_or("");
      match crate::dns::set_from_str(servers) {
        Ok(state) => {
          let payload = state.to_json_value();
          broadcaster.publish(EVENT_DNS_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, format!("dns set failed: {}", e)),
      }
    }
    crate::wired::OP_WIRED_LIST => match crate::wired::list() {
      Ok(interfaces) => success_frame(
        id,
        JsonValue::Object(vec![(
          "interfaces".to_string(),
          JsonValue::Array(interfaces.iter().map(|i| i.to_json_value()).collect()),
        )]),
      ),
      Err(e) => error_frame(id, format!("wired list failed: {}", e)),
    },
    crate::datetime::OP_DATETIME_GET => match store.lock() {
      Ok(guard) => success_frame(id, crate::datetime::get(&guard).to_json_value()),
      Err(_) => error_frame(id, "datetime get failed: store is locked".to_string()),
    },
    crate::datetime::OP_DATETIME_SET_TIMEZONE => {
      let timezone = params.get("timezone").and_then(|v| v.as_str()).unwrap_or("");
      match crate::datetime::set_timezone(store, timezone) {
        Ok(state) => {
          let payload = state.to_json_value();
          broadcaster.publish(EVENT_DATETIME_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::datetime::OP_DATETIME_SET_24H => {
      let use_24h = params.get("use_24h").and_then(|v| v.as_bool()).unwrap_or(false);
      match crate::datetime::set_24h(store, use_24h) {
        Ok(state) => {
          let payload = state.to_json_value();
          broadcaster.publish(EVENT_DATETIME_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::locale::OP_LOCALE_GET => match store.lock() {
      Ok(guard) => success_frame(id, crate::locale::get(&guard).to_json_value()),
      Err(_) => error_frame(id, "locale get failed: store is locked".to_string()),
    },
    crate::locale::OP_LOCALE_SET_LANGUAGE => {
      let language = params.get("language").and_then(|v| v.as_str()).unwrap_or("");
      match crate::locale::set_language(store, language) {
        Ok(state) => {
          let payload = state.to_json_value();
          broadcaster.publish(EVENT_LOCALE_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::locale::OP_LOCALE_SET_REGION => {
      let region = params.get("region").and_then(|v| v.as_str()).unwrap_or("");
      match crate::locale::set_region(store, region) {
        Ok(state) => {
          let payload = state.to_json_value();
          broadcaster.publish(EVENT_LOCALE_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::locale::OP_LOCALE_SET_KEYMAP => {
      let layout = params.get("layout").and_then(|v| v.as_str()).unwrap_or("");
      let variant = params.get("variant").and_then(|v| v.as_str());
      match crate::locale::set_keymap(store, layout, variant) {
        Ok(state) => {
          let payload = state.to_json_value();
          broadcaster.publish(EVENT_LOCALE_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::locale::OP_LOCALE_SET_AUTO_KEYMAP => {
      let auto = params.get("auto").and_then(|v| v.as_bool()).unwrap_or(false);
      match crate::locale::set_auto_keymap(store, auto) {
        Ok(state) => {
          let payload = state.to_json_value();
          broadcaster.publish(EVENT_LOCALE_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::locale::OP_LOCALE_KEYMAP_VARIANTS => {
      let layout = params.get("layout").and_then(|v| v.as_str()).unwrap_or("");
      success_frame(
        id,
        JsonValue::Object(vec![(
          "variants".to_string(),
          JsonValue::Array(
            crate::locale::keymap_variants(layout)
              .iter()
              .map(|v| JsonValue::Str(v.clone()))
              .collect(),
          ),
        )]),
      )
    }
    crate::customize::OP_CUSTOMIZE_GET => match store.lock() {
      Ok(guard) => success_frame(id, crate::customize::get(&guard).to_json_value()),
      Err(_) => error_frame(id, "customize get failed: store is locked".to_string()),
    },
    crate::customize::OP_CUSTOMIZE_SET => {
      let wallpaper = params.get("wallpaper").and_then(|v| v.as_str());
      let accent = params.get("accent").and_then(|v| v.as_str());
      let theme = params.get("theme").and_then(|v| v.as_str());
      let glass = params.get("glass").and_then(|v| v.as_str());
      match crate::customize::set(store, wallpaper, accent, theme, glass) {
        Ok(settings) => {
          let payload = settings.to_json_value();
          broadcaster.publish(EVENT_CUSTOMIZE_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_GET => match store.lock() {
      Ok(guard) => success_frame(id, crate::wallpaper::state(&guard).to_json_value()),
      Err(_) => error_frame(id, "wallpaper get failed: store is locked".to_string()),
    },
    crate::wallpaper::OP_WALLPAPER_SET_CURRENT => {
      let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("");
      let entry_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wallpaper::set_current(store, kind, entry_id) {
        Ok(entry) => {
          let payload = match entry {
            Some(entry) => entry.to_json_value(),
            None => JsonValue::Null,
          };
          broadcaster.publish(EVENT_WALLPAPER_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_SET_FILL => {
      let fill = params.get("fill").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wallpaper::set_fill(store, fill) {
        Ok(applied) => {
          let payload = JsonValue::Object(vec![("fill".to_string(), JsonValue::Str(applied))]);
          broadcaster.publish(EVENT_WALLPAPER_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_ADD => {
      let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
      let name = params.get("name").and_then(|v| v.as_str());
      match crate::wallpaper::add(Path::new(path), name) {
        Ok(entry) => {
          let payload = entry.to_json_value();
          broadcaster.publish(EVENT_WALLPAPER_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_APPLY => {
      let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("");
      let entry_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
      let variant = params.get("variant").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wallpaper::apply(store, kind, entry_id, variant) {
        Ok(entry) => {
          let payload = entry.to_json_value();
          broadcaster.publish(EVENT_WALLPAPER_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::wallpaper::OP_WALLPAPER_DELETE => {
      let entry_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
      match crate::wallpaper::delete(store, entry_id) {
        Ok(switched) => {
          let payload = JsonValue::Object(vec![
            ("deleted".to_string(), JsonValue::Bool(true)),
            ("switched".to_string(), JsonValue::Bool(switched)),
          ]);
          broadcaster.publish(EVENT_WALLPAPER_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    crate::display::OP_DISPLAY_GET => match store
      .lock()
      .map_err(|_| "display get failed: store is locked".to_string())
      .and_then(|guard| crate::display::get(&guard))
    {
      Ok(state) => success_frame(id, state.to_json_value()),
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
        Ok(state) => {
          let payload = state.to_json_value();
          broadcaster.publish(EVENT_DISPLAY_CHANGED, &payload);
          success_frame(id, payload)
        }
        Err(e) => error_frame(id, e),
      }
    }
    _ => error_frame(id, format!("unknown op: {:?}", op)),
  }
}

/// Read a `.fico` file from disk and return its content as JSON.
fn read_fico_json(path: &Path) -> Result<JsonValue, String> {
  let doc = sdk::FishFile::FishDocument::from_file(path).map_err(|e| e.to_string())?;
  JsonValue::parse(&doc.to_json()).map_err(|e| e.to_string())
}

fn success_frame(id: u64, result: JsonValue) -> JsonValue {
  crate::json::success_frame(id, result)
}

fn error_frame(id: u64, error: String) -> JsonValue {
  crate::json::error_frame(id, error)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use crate::json::JsonValue;

  fn no_params() -> JsonValue {
    JsonValue::Null
  }

  fn params(entries: Vec<(String, JsonValue)>) -> JsonValue {
    JsonValue::Object(entries)
  }

  fn str_param(key: &str, value: &str) -> (String, JsonValue) {
    (key.to_string(), JsonValue::Str(value.to_string()))
  }

  fn bool_param(key: &str, value: bool) -> (String, JsonValue) {
    (key.to_string(), JsonValue::Bool(value))
  }

  fn float_param(key: &str, value: f64) -> (String, JsonValue) {
    (key.to_string(), JsonValue::Float(value))
  }

  fn ok_of(frame: &JsonValue) -> Option<bool> {
    frame.get("ok")?.as_bool()
  }

  fn result_of<'a>(frame: &'a JsonValue) -> Option<&'a JsonValue> {
    frame.get("result")
  }

  fn error_text(frame: &JsonValue) -> String {
    frame.get("error").and_then(|v| v.as_str()).unwrap_or("").to_string()
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

  fn bc() -> Broadcaster {
    Broadcaster::new()
  }

  #[test]
  fn ping_frame() {
    let store = memory_store();
    let frame = dispatch(7, OP_PING, &no_params(), Path::new("/nonexistent"), Path::new("/nonexistent"), &store, &bc(), &mut None);
    assert_eq!(frame.get("id").and_then(|v| v.as_i64()), Some(7));
    assert_eq!(ok_of(&frame), Some(true));
    assert_eq!(result_of(&frame).and_then(|r| r.get("pong")).and_then(|v| v.as_bool()), Some(true));
  }

  #[test]
  fn unknown_op_frame() {
    let store = memory_store();
    let frame = dispatch(3, "delete_everything", &no_params(), Path::new("/nonexistent"), Path::new("/nonexistent"), &store, &bc(), &mut None);
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("unknown op"));
  }

  #[test]
  fn invalid_line_frame() {
    let store = memory_store();
    let frame = handle_line("not json\n", Path::new("/nonexistent"), Path::new("/nonexistent"), &store, &bc(), &mut None);
    assert_eq!(frame.get("id").and_then(|v| v.as_i64()), Some(0));
    assert_eq!(ok_of(&frame), Some(false));
  }

  #[test]
  fn missing_file_frame() {
    let store = memory_store();
    let frame = dispatch(1, OP_GET_OS, &no_params(), Path::new("/nonexistent"), Path::new("/nonexistent-sys.fico"), &store, &bc(), &mut None);
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("os.fico unavailable"));
  }

  #[test]
  fn wifi_connect_rejects_missing_ssid() {
    let store = memory_store();
    let frame = dispatch(
      4,
      crate::wifi::OP_WIFI_CONNECT,
      &JsonValue::Object(vec![]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("wifi connect failed"));
  }

  #[test]
  fn wifi_forget_rejects_missing_ssid() {
    let store = memory_store();
    let frame = dispatch(
      5,
      crate::wifi::OP_WIFI_FORGET,
      &JsonValue::Object(vec![]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("wifi forget failed"));
  }

  #[test]
  fn dns_set_rejects_invalid_servers() {
    let store = memory_store();
    let frame = dispatch(
      7,
      crate::dns::OP_DNS_SET,
      &params(vec![str_param("servers", "not-an-ip")]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("dns set failed"));
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
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(true));
    let result = result_of(&frame).unwrap();
    assert_eq!(result.get("wallpaper").and_then(|v| v.as_str()), Some(crate::customize::DEFAULT_WALLPAPER));
    assert_eq!(result.get("accent").and_then(|v| v.as_str()), Some(crate::customize::DEFAULT_ACCENT));
    assert_eq!(result.get("theme").and_then(|v| v.as_str()), Some(crate::customize::DEFAULT_THEME));
  }

  #[test]
  fn customize_set_roundtrip_and_rejects_invalid() {
    let store = memory_store();
    let frame = dispatch(
      7,
      crate::customize::OP_CUSTOMIZE_SET,
      &params(vec![
        str_param("wallpaper", "SONOMA"),
        str_param("accent", "blue"),
        str_param("theme", "light"),
      ]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(true));
    assert_eq!(result_of(&frame).and_then(|r| r.get("wallpaper")).and_then(|v| v.as_str()), Some("SONOMA"));

    let frame = dispatch(
      8,
      crate::customize::OP_CUSTOMIZE_GET,
      &no_params(),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(result_of(&frame).and_then(|r| r.get("accent")).and_then(|v| v.as_str()), Some("blue"));

    let frame = dispatch(
      9,
      crate::customize::OP_CUSTOMIZE_SET,
      &params(vec![str_param("theme", "sepia")]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("unknown theme"));
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
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(true));
    let result = result_of(&frame).unwrap();
    assert!(result.get("premade").and_then(|v| v.as_array()).is_some());
    assert!(result.get("customs").and_then(|v| v.as_array()).is_some());
    assert_eq!(result.get("fill").and_then(|v| v.as_str()), Some(crate::wallpaper::DEFAULT_FILL));
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
      &params(vec![str_param("fill", "tile")]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(true));
    assert_eq!(result_of(&frame).and_then(|r| r.get("fill")).and_then(|v| v.as_str()), Some("tile"));
    let frame = dispatch(
      12,
      crate::wallpaper::OP_WALLPAPER_SET_FILL,
      &params(vec![str_param("fill", "melt")]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("unknown fill mode"));
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
      &params(vec![str_param("kind", "orb"), str_param("id", "FLOW")]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("unknown kind"));
  }

  #[test]
  fn wallpaper_add_rejects_missing_file() {
    let store = memory_store();
    let frame = dispatch(
      14,
      crate::wallpaper::OP_WALLPAPER_ADD,
      &params(vec![str_param("path", "/nonexistent-wallpaper-test/missing.png")]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("file not found"));
  }

  #[test]
  fn wallpaper_apply_rejects_unknown_variant() {
    let store = memory_store();
    let frame = dispatch(
      15,
      crate::wallpaper::OP_WALLPAPER_APPLY,
      &params(vec![
        str_param("kind", "premade"),
        str_param("id", "FLOW"),
        str_param("variant", "sepia"),
      ]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("unknown variant"));
  }

  #[test]
  fn wallpaper_delete_rejects_unknown_id() {
    let store = memory_store();
    let frame = dispatch(
      16,
      crate::wallpaper::OP_WALLPAPER_DELETE,
      &params(vec![str_param("id", "ghost")]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("unknown custom wallpaper"));
  }

  #[test]
  fn display_set_validates_before_touching_anything() {
    let store = memory_store();
    let frame = dispatch(
      17,
      crate::display::OP_DISPLAY_SET,
      &params(vec![float_param("brightness", 120.0)]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("invalid brightness"));
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
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("unreachable"));
    std::env::remove_var("COMPOSITOR_SOCKET");
  }

  /// Mock compositor socket replying canned frames in order, one per
  /// connection.
  fn mock_display(replies: Vec<JsonValue>) -> PathBuf {
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
        let mut out = reply.stringify(false);
        out.push('\n');
        let _ = stream.write_all(out.as_bytes());
      }
    });
    path
  }

  fn compositor_ok_frame(entries: Vec<(String, JsonValue)>) -> JsonValue {
    JsonValue::Object(vec![
      ("ok".to_string(), JsonValue::Bool(true)),
      ("result".to_string(), JsonValue::Object(entries)),
    ])
  }

  #[test]
  fn display_set_roundtrip_persists() {
    let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
    let sock = mock_display(vec![
      compositor_ok_frame(vec![
        ("output".to_string(), JsonValue::Str("HDMI-1".to_string())),
        (
          "mode".to_string(),
          JsonValue::Object(vec![
            ("width".to_string(), JsonValue::Integer(1920)),
            ("height".to_string(), JsonValue::Integer(1080)),
            ("refresh".to_string(), JsonValue::Integer(120)),
          ]),
        ),
        ("brightness".to_string(), JsonValue::Integer(80)),
        ("night_light".to_string(), JsonValue::Bool(true)),
      ]),
      compositor_ok_frame(vec![("outputs".to_string(), JsonValue::Array(vec![]))]),
    ]);
    std::env::set_var("COMPOSITOR_SOCKET", &sock);
    let store = memory_store();
    let frame = dispatch(
      19,
      crate::display::OP_DISPLAY_SET,
      &params(vec![float_param("brightness", 80.0), bool_param("night_light", true)]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store, &bc(), &mut None,
    );
    assert_eq!(ok_of(&frame), Some(true));
    let result = result_of(&frame).unwrap();
    assert_eq!(result.get("brightness").and_then(|v| v.as_i64()), Some(80));
    assert_eq!(result.get("night_light").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
      result.get("outputs"),
      Some(&JsonValue::Array(vec![]))
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
    std::fs::write(&os_path, "os {\n    version: \"27.0.0\"\n    beta: false\n}\n").unwrap();
    let sys_path = dir.join("sys.fico");
    std::fs::write(&sys_path, "ram {\n    total_gb: 32.0\n}\n").unwrap();

    let (client, server) = UnixStream::pair().unwrap();
    let sys_clone = sys_path.clone();
    let os_clone = os_path.clone();
    let store = Arc::new(Mutex::new(SettingsStore::new(
      dir.join("socket-test-settings.json"),
    )));
    std::thread::spawn(move || serve_connection(server, sys_clone, os_clone, store, Broadcaster::shared()));

    let mut writer = client.try_clone().unwrap();
    let mut reader = BufReader::new(client);
    let mut line = String::new();

    writer.write_all(b"{\"id\": 1, \"op\": \"get_os\"}\n").unwrap();
    writer.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    let frame = JsonValue::parse(&line).unwrap();
    assert_eq!(ok_of(&frame), Some(true));
    assert_eq!(
      frame.get("result").and_then(|r| r.get("os")).and_then(|o| o.get("version")).and_then(|v| v.as_str()),
      Some("27.0.0")
    );

    line.clear();
    writer.write_all(b"{\"id\": 2, \"op\": \"get_hardware\"}\n").unwrap();
    writer.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    let frame = JsonValue::parse(&line).unwrap();
    assert_eq!(
      frame.get("result").and_then(|r| r.get("ram")).and_then(|o| o.get("total_gb")).and_then(|v| v.as_f64()),
      Some(32.0)
    );

    let _ = std::fs::remove_file(&os_path);
    let _ = std::fs::remove_file(&sys_path);
  }

  #[test]
  fn subscribe_reports_filter_and_registers_nothing_by_itself() {
    // dispatch alone never touches the registry: attaching the connection
    // is the connection loop's job (it owns the stream).
    let store = memory_store();
    let bc = bc();
    let mut pending = None;
    let frame = dispatch(
      1,
      OP_SUBSCRIBE,
      &params(vec![str_param("events", "x")]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
      &bc,
      &mut pending,
    );
    // Wrong shape on purpose: events must be an array, not a string.
    assert_eq!(ok_of(&frame), Some(false));
    assert!(error_text(&frame).contains("events must be an array"));
    assert_eq!(pending, None);
    assert_eq!(bc.subscriber_count(), 0);
  }

  #[test]
  fn subscribe_accepts_filter_and_empty_means_all() {
    let store = memory_store();
    let bc = bc();
    let mut pending = None;
    let frame = dispatch(
      2,
      OP_SUBSCRIBE,
      &params(vec![(
        "events".to_string(),
        JsonValue::Array(vec![JsonValue::Str(EVENT_CUSTOMIZE_CHANGED.to_string())]),
      )]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
      &bc,
      &mut pending,
    );
    assert_eq!(ok_of(&frame), Some(true));
    assert_eq!(
      pending,
      Some(vec![EVENT_CUSTOMIZE_CHANGED.to_string()])
    );

    let mut pending = None;
    let frame = dispatch(
      3,
      OP_SUBSCRIBE,
      &JsonValue::Object(vec![]),
      Path::new("/nonexistent"),
      Path::new("/nonexistent"),
      &store,
      &bc,
      &mut pending,
    );
    assert_eq!(ok_of(&frame), Some(true));
    assert_eq!(pending, Some(vec![]));
  }

  #[test]
  fn publish_reaches_matching_subscribers_only() {
    let bc = Broadcaster::new();
    let (_, filtered_rx) = bc.register(vec![EVENT_CUSTOMIZE_CHANGED.to_string()]);
    let (_, all_rx) = bc.register(vec![]);
    bc.publish(
      EVENT_CUSTOMIZE_CHANGED,
      &JsonValue::Object(vec![("theme".to_string(), JsonValue::Str("light".to_string()))]),
    );
    bc.publish(
      EVENT_WIFI_CHANGED,
      &JsonValue::Object(vec![("enabled".to_string(), JsonValue::Bool(true))]),
    );
    let first = JsonValue::parse(&filtered_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap()).unwrap();
    assert_eq!(first.get("event").and_then(|v| v.as_str()), Some(EVENT_CUSTOMIZE_CHANGED));
    assert_eq!(
      first.get("result").and_then(|r| r.get("theme")).and_then(|v| v.as_str()),
      Some("light")
    );
    assert!(filtered_rx.recv_timeout(std::time::Duration::from_millis(100)).is_err());

    let mut names = Vec::new();
    for _ in 0..2 {
      let frame = JsonValue::parse(&all_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap()).unwrap();
      names.push(frame.get("event").and_then(|v| v.as_str()).unwrap().to_string());
    }
    names.sort();
    assert_eq!(names, vec![EVENT_CUSTOMIZE_CHANGED.to_string(), EVENT_WIFI_CHANGED.to_string()]);
  }

  #[test]
  fn dead_subscribers_are_dropped() {
    let bc = Broadcaster::new();
    let (_, rx) = bc.register(vec![]);
    assert_eq!(bc.subscriber_count(), 1);
    drop(rx);
    bc.publish(EVENT_DNS_CHANGED, &JsonValue::Null);
    assert_eq!(bc.subscriber_count(), 0);
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn subscribed_connection_receives_customize_event() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let dir = std::env::temp_dir().join("settings-daemon-subscribe-test");
    let _ = std::fs::create_dir_all(&dir);
    let (client, server) = UnixStream::pair().unwrap();
    let store = Arc::new(Mutex::new(SettingsStore::new(
      dir.join("socket-test-settings.json"),
    )));
    let bc = Broadcaster::shared();
    let bc_clone = bc.clone();
    std::thread::spawn(move || {
      serve_connection(
        server,
        PathBuf::from("/nonexistent"),
        PathBuf::from("/nonexistent"),
        store,
        bc_clone,
      )
    });

    let mut writer = client.try_clone().unwrap();
    let mut reader = BufReader::new(client);
    let mut line = String::new();
    writer
      .write_all(b"{\"id\": 1, \"op\": \"subscribe\", \"params\": {\"events\": [\"customize_changed\"]}}\n")
      .unwrap();
    writer.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    let ack = JsonValue::parse(&line).unwrap();
    assert_eq!(ok_of(&ack), Some(true));
    assert_eq!(bc.subscriber_count(), 1);

    bc.publish(
      EVENT_CUSTOMIZE_CHANGED,
      &JsonValue::Object(vec![
        ("theme".to_string(), JsonValue::Str("light".to_string())),
        ("revision".to_string(), JsonValue::Integer(9)),
      ]),
    );
    line.clear();
    reader.read_line(&mut line).unwrap();
    let event = JsonValue::parse(&line).unwrap();
    assert_eq!(event.get("event").and_then(|v| v.as_str()), Some(EVENT_CUSTOMIZE_CHANGED));
    assert_eq!(
      event.get("result").and_then(|r| r.get("revision")).and_then(|v| v.as_i64()),
      Some(9)
    );
    drop(writer);
    drop(reader);
    // Give the connection loop a moment to notice EOF and unregister.
    for _ in 0..50 {
      if bc.subscriber_count() == 0 {
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(bc.subscriber_count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
  }
}
