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
// Request:  {"id": 1, "op": "ping" | "get_hardware" | "get_os"}
// Success:  {"id": 1, "ok": true, "result": {...}}
// Failure:  {"id": 1, "ok": false, "error": "..."}
//
// `get_hardware` returns the parsed `sys.fico` content, `get_os` the parsed
// `os.fico` content. Missing or corrupt files return `ok: false`, never a
// partial document.

pub const OP_PING: &str = "ping";
pub const OP_GET_HARDWARE: &str = "get_hardware";
pub const OP_GET_OS: &str = "get_os";

/// Socket server with the read protocol. `store` and `libs` are accepted for
/// future write dispatch and currently unused.
#[cfg(target_os = "linux")]
pub fn run_server(
  config: &DaemonConfig,
  _store: Arc<Mutex<SettingsStore>>,
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
        std::thread::spawn(move || serve_connection(stream, sys, os));
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
fn serve_connection(stream: std::os::unix::net::UnixStream, sys: PathBuf, os: PathBuf) {
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
    let frame = handle_line(&line, &sys, &os);
    let mut out = frame.to_string();
    out.push('\n');
    if writer.write_all(out.as_bytes()).is_err() || writer.flush().is_err() {
      break;
    }
  }
}

/// Parse one request line and dispatch it. Never panics, always returns a
/// frame with the request `id` (0 when the line has no usable id).
fn handle_line(line: &str, sys: &Path, os: &Path) -> serde_json::Value {
  let request: serde_json::Value = match serde_json::from_str(line) {
    Ok(value) => value,
    Err(e) => return error_frame(0, format!("invalid request: {}", e)),
  };
  let id = request.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
  let op = request.get("op").and_then(|v| v.as_str()).unwrap_or("");
  dispatch(id, op, sys, os)
}

fn dispatch(id: u64, op: &str, sys: &Path, os: &Path) -> serde_json::Value {
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

  #[test]
  fn ping_frame() {
    let frame = dispatch(7, OP_PING, Path::new("/nonexistent"), Path::new("/nonexistent"));
    assert_eq!(frame["id"], 7);
    assert_eq!(frame["ok"], true);
    assert_eq!(frame["result"]["pong"], true);
  }

  #[test]
  fn unknown_op_frame() {
    let frame = dispatch(3, "delete_everything", Path::new("/nonexistent"), Path::new("/nonexistent"));
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("unknown op"));
  }

  #[test]
  fn invalid_line_frame() {
    let frame = handle_line("not json\n", Path::new("/nonexistent"), Path::new("/nonexistent"));
    assert_eq!(frame["id"], 0);
    assert_eq!(frame["ok"], false);
  }

  #[test]
  fn missing_file_frame() {
    let frame = dispatch(1, OP_GET_OS, Path::new("/nonexistent"), Path::new("/nonexistent-sys.fico"));
    assert_eq!(frame["ok"], false);
    assert!(frame["error"].as_str().unwrap().contains("os.fico unavailable"));
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
    std::thread::spawn(move || serve_connection(server, sys_clone, os_clone));

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
