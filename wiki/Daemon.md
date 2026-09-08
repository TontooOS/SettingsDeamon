# Daemon

The `settings-daemon` binary loads `DaemonConfig`, restores the
`SettingsStore` from disk, refreshes the hardware snapshot in `sys.fico` and
the OS identity in `os.fico`, creates an empty `LibraryManager` and runs the
socket server with the read protocol (`ping`, `get_hardware`, `get_os`).
Write dispatch is still a roadmap item.

## Configuration

### `DaemonConfig`

```rust
pub struct DaemonConfig {
  pub socket_path: PathBuf,
  pub store_path: PathBuf,
  pub sys_fico_path: PathBuf,
  pub os_fico_path: PathBuf,
}
```

| Field | Type | Description |
|---|---|---|
| `socket_path` | `PathBuf` | Unix socket path, defaults to `/run/tontoo-settings.sock` |
| `store_path` | `PathBuf` | Settings file, defaults to `/Library/Preferences/TontooSettings/settings.json` |
| `sys_fico_path` | `PathBuf` | Hardware snapshot, defaults to `/Library/Preferences/SystemConfiguration/sys.fico` |
| `os_fico_path` | `PathBuf` | OS identity, defaults to `/Library/Preferences/SystemConfiguration/os.fico` |

### Constructors

```rust
impl DaemonConfig {
  pub fn from_env() -> Self;
}
```

Reads `SETTINGS_SOCKET`, `SETTINGS_STORE`, `SETTINGS_SYS_FICO` and
`SETTINGS_OS_FICO`, falls back to `Default` when unset. Returns a config,
never fails.

```rust
let config = DaemonConfig::from_env();
```

## Runtime Model

1. Init logging via `env_logger` (`RUST_LOG` overrides, default `info`).
2. Load store with `SettingsStore::load`. Missing file starts empty with a
   warning, corrupt file logs an error and exits non-zero only when the
   socket fails, otherwise the daemon keeps running with an empty store.
3. Refresh hardware with `hardware::write_sys_fico(&config.sys_fico_path,
   true)`. A failing refresh is logged as a warning and never stops the
   daemon, so the socket still comes up on unknown hardware.
4. Refresh OS identity with `os::write_os_fico(&config.os_fico_path)`.
   Same failure policy as the hardware refresh.
5. Wrap store and library manager in `Arc<Mutex<..>>` for later threads.
6. Block in `socket::run_server`. Any `Err` is logged and the process exits
   with status `1`.

## Usage / Example

```bash
RUST_LOG=debug SETTINGS_STORE=/tmp/settings.json SETTINGS_SYS_FICO=/tmp/sys.fico SETTINGS_OS_FICO=/tmp/os.fico settings-daemon
```

## Cross References

- [Store.md](Store.md) – persistence behind the loaded store
- [Library.md](Library.md) – registry created at startup
- [Hardware.md](Hardware.md) – snapshot refreshed at startup
- [Os.md](Os.md) – OS identity refreshed at startup
- [Socket.md](Socket.md) – server the daemon blocks in
