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
| `customize_get` | Effective customization (`{"wallpaper", "accent", "theme"}`) |
| `customize_set` | Private: partial `{"wallpaper"?, "accent"?, "theme"?}` returns the effective settings |
| `wallpaper_get` | Full wallpaper state (`{"current", "fill", "customs", "premade"}`) |
| `wallpaper_set_current` | Private: `{"kind", "id"}` returns the selected entry |
| `wallpaper_set_fill` | Private: `{"fill"}` returns `{"fill"}` |
| `wallpaper_add` | Private: `{"path", "name"?}` converts to PNG and returns the new entry |
| `wallpaper_apply` | Private: `{"kind", "id", "variant"}` applies to the desktop, returns the applied entry |
| `wallpaper_delete` | Private: `{"id"}` deletes a custom (pre-switch to Tahoe Lake when current), returns `{"deleted", "switched"}` |

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
- `customize_set` follows the same visibility rule as the `wifi_*` write
  ops (Settings app only); `customize_get` is public. Invalid values are
  rejected with `ok: false` and leave the store untouched.
- `wallpaper_set_current`, `wallpaper_set_fill` and `wallpaper_add`
  follow the same visibility rule (Settings app only);
  `wallpaper_get` is public. `wallpaper_apply` (Settings app only)
  resolves the variant file, forwards it to the compositor for the
  desktop crossfade, then persists the selection. `wallpaper_delete`
  removes a user custom (pre-switch to Tahoe Lake when current).

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
  serves the read protocol there. Each thread shares the store, so
  `customize_set` writes are visible to every client.
- Returns `Ok` only when the loop ends, which currently means never during
  normal operation.
- On non-Linux targets returns `Err` with kind `Unsupported`.

```rust
pub const OP_PING: &str = "ping";
pub const OP_GET_HARDWARE: &str = "get_hardware";
pub const OP_GET_OS: &str = "get_os";
```

WiFi op names live with the backend (`settings_daemon::wifi::OP_WIFI_*`):
`wifi_list`, `wifi_status` (public) and `wifi_connect`,
`wifi_disconnect`, `wifi_enable`, `wifi_disable`, `wifi_forget`
(private, Settings app only). Customize op names live with the backend
(`settings_daemon::customize::OP_CUSTOMIZE_*`): `customize_get` (public)
and `customize_set` (private, Settings app only). Wallpaper op names live
with the backend (`settings_daemon::wallpaper::OP_WALLPAPER_*`):
`wallpaper_get` (public) and `wallpaper_set_current`,
`wallpaper_set_fill`, `wallpaper_add`, `wallpaper_apply`,
`wallpaper_delete` (private, Settings app only).
Request params arrive as an optional `params` object next to `id` and
`op`.

## Roadmap

| Step | Description |
|---|---|
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
- [Store.md](Store.md) – backing data for the customize dispatch
- [Customize.md](Customize.md) – `customize` domain, validation and ops
- [Wallpaper.md](Wallpaper.md) – wallpaper state, customs registry and ops
- [Library.md](Library.md) – registry for future dispatch
