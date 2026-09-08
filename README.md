# SettingsDeamon

Settings daemon basis for TontooOS. It owns the system settings store,
manages settings libraries, refreshes the hardware snapshot in `sys.fico`
and the OS identity in `os.fico` on every start and serves clients over a
unix socket.

Current state is the basis plus the system backends: `DaemonConfig`,
`SettingsStore` with JSON persistence, `LibraryManager` registry stub,
`hardware` module (processor, GPU, RAM via FishFile), `os` module (name,
version, beta flag via FishFile) and the socket read protocol
(`ping`/`get_hardware`/`get_os`, used by the SettingsProvider library).
Write dispatch, client library and C API are roadmap items.

## Made for TontooOS

Explore more at https://github.com/TontooOS/Libs

## Documentation

See [wiki/MAIN.md](wiki/MAIN.md) for the full wiki.

## Quick Start

```bash
cargo run --bin settings-daemon
```

```bash
SETTINGS_SOCKET=/tmp/tontoo-settings.sock SETTINGS_STORE=/tmp/settings.json SETTINGS_SYS_FICO=/tmp/sys.fico SETTINGS_OS_FICO=/tmp/os.fico cargo run --bin settings-daemon
```

## Layout

```text
src/
  lib.rs       # crate root, re-exports
  config.rs    # DaemonConfig + defaults + env
  store.rs     # SettingsStore (domains, keys, JSON file)
  library.rs   # LibraryManager stub (register/list/unregister)
  hardware.rs  # hardware backend (CPU/GPU/RAM -> sys.fico)
  os.rs        # OS identity backend (name/version/beta -> os.fico)
  socket.rs    # socket read protocol (ping/get_hardware/get_os)
  main.rs      # binary: load store, refresh sys.fico + os.fico, run socket server
lang/
  en_us.json
  de_de.json
Headers/
  settings_daemon.h   # future C API stub
wiki/
  MAIN.md
```

## License

TCL v26.1