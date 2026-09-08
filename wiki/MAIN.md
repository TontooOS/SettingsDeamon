# SettingsDeamon – Wiki

SettingsDeamon is the TontooOS settings service. It owns the system settings
store, manages settings libraries, refreshes the hardware snapshot in
`sys.fico` and the OS identity in `os.fico` on every start and serves
clients over a unix socket.
This repository holds the daemon basis plus the system backends:
configuration, in-memory store with JSON persistence, library registry stub,
hardware detection (processor, GPU, RAM), OS identity and socket accept stub.

- Repository: https://github.com/TontooOS/SettingsDeamon
- License: TCL
- Version: 0.1.0

## Feature Index

| Feature | File | Description |
|---|---|---|
| Main index | [MAIN.md](MAIN.md) | This page |
| Rules | [RULE.md](RULE.md) | Development and usage rules |
| Daemon | [Daemon.md](Daemon.md) | `settings-daemon` binary, configuration and runtime model |
| Store | [Store.md](Store.md) | `SettingsStore`, domains, keys and JSON persistence |
| Library | [Library.md](Library.md) | `LibraryManager`, settings library registry stub |
| Hardware | [Hardware.md](Hardware.md) | Hardware backend, `sys.fico` snapshot of processor, GPU, RAM |
| Os | [Os.md](Os.md) | OS identity backend, `os.fico` with name, version and beta flag |
| Socket | [Socket.md](Socket.md) | Unix socket server basis and protocol roadmap |

## Quick Start

Run the daemon with defaults:

```bash
settings-daemon
```

Override paths via environment:

```bash
SETTINGS_SOCKET=/tmp/tontoo-settings.sock SETTINGS_STORE=/tmp/settings.json SETTINGS_SYS_FICO=/tmp/sys.fico SETTINGS_OS_FICO=/tmp/os.fico settings-daemon
```

Embed the store in Rust:

```rust
use settings_daemon::SettingsStore;
use std::path::PathBuf;

let mut store = SettingsStore::new(PathBuf::from("/tmp/settings.json"));
store.set("appearance", "theme", serde_json::json!("dark"));
store.save()?;
```

Write a hardware snapshot in Rust:

```rust
use settings_daemon::hardware;
use std::path::Path;

let info = hardware::write_sys_fico(Path::new("/tmp/sys.fico"), true)?;
```

See [Daemon.md](Daemon.md) for details.

## Changelog

- 2026-09-07: Read protocol. `ping`/`get_hardware`/`get_os` over
  newline-delimited JSON, served per connection thread. Used by the
  SettingsProvider client library.
- 2026-09-07: OS identity backend. `os` module (display name, codename,
  version, beta flag via `/etc/tontoo-release` plus env overrides),
  `os.fico` refresh at startup, `SETTINGS_OS_FICO` config key.

- 2026-09-07: Hardware backend. `hardware` module (CPU via `/proc/cpuinfo`,
  RAM size via `/proc/meminfo`, RAM type and modules via `dmidecode`,
  GPUs via DRM sysfs plus `lspci`), `sys.fico` refresh at startup,
  `SETTINGS_SYS_FICO` config key, `sdk`/`FishFile` dependency.
- 2026-09-07: Initial basis. `DaemonConfig`, `SettingsStore`, `LibraryManager`
  stub, socket accept stub, `lang/` files, `Headers/settings_daemon.h` stub.
