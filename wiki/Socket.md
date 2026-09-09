# Socket

The socket server binds `DaemonConfig::socket_path` and serves the read
protocol over newline-delimited JSON. Each connection handles any number of
requests until EOF. Write operations (`set`/`subscribe`/events) are roadmap
items and will be dispatched here, following the FishPerms daemon pattern.

## Protocol

One JSON object per line, one reply line per request.

Request:

```json
{"id": 1, "op": "ping"}
```

| Op | Result |
|---|---|
| `ping` | `{"pong": true}` |
| `get_hardware` | Parsed `sys.fico` content (`processor`, `gpu0`..`gpuN`, `ram`) |
| `get_os` | Parsed `os.fico` content (`os`) |
| `wifi_list` | Nearby networks with `known` flag (`{"networks": [...]}`) |
| `wifi_status` | Radio state plus active connection (`{"enabled": bool, "status": ...\|null}`) |
| `wifi_connect` | Private: `{"ssid", "password"?, "hidden"?}` returns the verified status |
| `wifi_disconnect` | Private: `{"disconnected": true}` |
| `wifi_enable` | Private: `{"enabled": true}` |
| `wifi_disable` | Private: `{"enabled": false}` |
| `wifi_forget` | Private: `{"ssid"}` returns `{"forgotten": bool}` |

Success reply:

```json
{"id": 1, "ok": true, "result": {"os": {"version": "26.1.0"}}}
```

Failure reply (unknown op, missing or corrupt file):

```json
{"id": 1, "ok": false, "error": "unknown op: \"reset\""}
```

Rules:

- The reply always carries the request `id` (`0` when the line has no
  usable id).
- Malformed lines get an error frame, the connection stays open.
- Missing or corrupt fico files return `ok: false`, never partial data.
- The `wifi_*` write ops (`connect`, `disconnect`, `enable`, `disable`,
  `forget`) are private: no public client library exposes them, only the
  Settings app (`com.tontoo.systemsettings`) may call them. `wifi_list`
  and `wifi_status` are public read ops served to every client.

## API

```rust
pub fn run_server(
  config: &DaemonConfig,
  store: Arc<Mutex<SettingsStore>>,
  libs: Arc<Mutex<LibraryManager>>,
) -> io::Result<()>;
```

- Binds the unix socket after removing a stale file. Returns `Err` when the
  bind fails.
- Loops over `listener.incoming()`, spawns one thread per connection and
  serves the read protocol there.
- Returns `Ok` only when the loop ends, which currently means never during
  normal operation.
- On non-Linux targets returns `Err` with kind `Unsupported`.
- `store` and `libs` are accepted for future write dispatch and currently
  unused.

```rust
pub const OP_PING: &str = "ping";
pub const OP_GET_HARDWARE: &str = "get_hardware";
pub const OP_GET_OS: &str = "get_os";
```

WiFi op names live with the backend (`settings_daemon::wifi::OP_WIFI_*`):
`wifi_list`, `wifi_status` (public) and `wifi_connect`,
`wifi_disconnect`, `wifi_enable`, `wifi_disable`, `wifi_forget`
(private, Settings app only). Request params arrive as an optional
`params` object next to `id` and `op`.

## Roadmap

| Step | Description |
|---|---|
| Store dispatch | `get`/`set` frames on `SettingsStore` |
| Library dispatch | Library register/reload frames |
| Permissions | Peer credential check before writes |
| Events | `subscribe` plus change events |

## Usage / Example

```bash
printf '{"id": 1, "op": "get_os"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
```

```rust
use settings_daemon::{DaemonConfig, LibraryManager, SettingsStore};
use std::sync::{Arc, Mutex};

let config = DaemonConfig::from_env();
let store = Arc::new(Mutex::new(SettingsStore::new(config.store_path.clone())));
let libs = Arc::new(Mutex::new(LibraryManager::new()));
settings_daemon::socket::run_server(&config, store, libs)?;
```

## Cross References

- [Daemon.md](Daemon.md) – configuration and runtime model
- [Hardware.md](Hardware.md) – content behind `get_hardware`
- [Os.md](Os.md) – content behind `get_os`
- [Store.md](Store.md) – backing data for future dispatch
- [Library.md](Library.md) – registry for future dispatch
