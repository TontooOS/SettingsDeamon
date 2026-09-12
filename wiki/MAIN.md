# SettingsDeamon – Wiki

SettingsDeamon is the TontooOS settings service. It owns the system settings
store, manages settings libraries, refreshes the hardware snapshot in
`sys.fico` and the OS identity in `os.fico` on every start and serves
clients over a unix socket.
This repository holds the daemon basis plus the system backends:
configuration, in-memory store with JSON persistence, library registry stub,
hardware detection (processor, GPU, RAM), OS identity, WiFi control with
known networks in CoreData and the socket server.

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
| WiFi | [Wifi.md](Wifi.md) | WiFi backend, known networks in CoreData, public/private socket ops |
| Customize | [Customize.md](Customize.md) | Customize backend, `customize` store domain, public/private socket ops |
| Wallpaper | [Wallpaper.md](Wallpaper.md) | Wallpaper backend, premade packs, customs registry, public/private socket ops |
| Display | [Display.md](Display.md) | Display backend, `display` store domain, public/private socket ops |
| Dns | [Dns.md](Dns.md) | DNS backend, active-connection IPv4 servers, public/private socket ops |
| Wired | [Wired.md](Wired.md) | Wired backend, connected Ethernet interfaces with details, public socket op |

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

- 2026-09-12: Wired backend. `wired` module (connected Ethernet
  interfaces with addresses from NetworkManager plus MAC/MTU/speed/
  driver from sysfs), socket op `wired_list` (public). See
  [Wired.md](Wired.md).
- 2026-09-12: DNS backend. `dns` module (IPv4 servers of the active
  NetworkManager connection, strict validation, empty means DHCP),
  socket ops `dns_get` (public) plus `dns_set` (private, Settings app
  only; reactivates the connection so the change applies live). See
  [Dns.md](Dns.md).
- 2026-09-12: WiFi system store and auto-join. Known networks (with
  encrypted passwords) move to `/System/Preferences/com.tontoo.wifi/storage.fico`;
  new public op `wifi_known_list` (never exposes passwords); `wifi_status`
  gains `available`; daemon auto-joins the most recently used visible
  known network at startup. See [Wifi.md](Wifi.md).
- 2026-09-11: Display backend. `display` module (`display` store
  domain: brightness, night light, per-output modes), socket ops
  `display_get` (public) plus `display_set` (private, Settings app
  only; forwards live, then persists), startup push. Shared test env
  lock across modules (parallel env races). See [Display.md](Display.md).
- 2026-09-11: Fill mode goes live. `apply`, boot push and `set_fill`
  forward the fill mode to the compositor (`set_wallpaper` frame);
  `set_fill` persists only after a successful forward (persist-only
  with no wallpaper configured). See [Wallpaper.md](Wallpaper.md).
- 2026-09-11: Wallpaper delete. `wallpaper_delete` op (customs only,
  never premade): pre-switches to Tahoe Lake (auto) when the deleted
  wallpaper is current, then removes the file, the CoreData entry and
  refreshes `storage.fico`. See [Wallpaper.md](Wallpaper.md).
- 2026-09-11: Wallpaper apply. `wallpaper_apply` op (light/dark/auto
  variant, `auto` follows the `customize` theme) forwards the resolved
  file to the compositor socket for the desktop crossfade, then persists
  the selection; entries carry `path_dark`. See [Wallpaper.md](Wallpaper.md).
- 2026-09-11: Wallpaper backend. `wallpaper` module (premade packs from
  `/System/User/Wallpapers`, customs in
  `~/Library/Preferences/com.tontoo.wallpaper/` as PNG with CoreData
  `CustomWallpaper` registry plus `storage.fico` mirror, current/fill in
  the `wallpaper` store domain), socket ops `wallpaper_get` (public)
  plus `wallpaper_set_current`/`wallpaper_set_fill`/`wallpaper_add`
  (private, Settings app only; selection persistence only, no desktop
  apply). See [Wallpaper.md](Wallpaper.md).
- 2026-09-11: Customize backend. `customize` module (`customize` store
  domain with `wallpaper`/`accent`/`theme`, validated read overlay and
  partial-write `set` with persistence), socket ops `customize_get`
  (public) plus `customize_set` (private, Settings app only); the store
  is now shared across connection threads. See [Customize.md](Customize.md).
- 2026-09-09: WiFi backend. `wifi` module (scan with known flag, status,
  connect, disconnect, enable, disable, forget), known networks in
  CoreData (`KnownWifi` under `com.tontoo.settingsdaemon`), socket ops
  `wifi_list`/`wifi_status` (public) plus `wifi_connect`/`wifi_disconnect`/
  `wifi_enable`/`wifi_disable`/`wifi_forget` (private, Settings app only).
  NetworkKit `Wifi` is read-only and queries the daemon first.
- 2026-09-07: Read protocol. `ping`/`get_hardware`/`get_os` over
  newline-delimited JSON, served per connection thread. Used by the
  CoreSettings client library.
- 2026-09-07: OS identity backend. `os` module (display name, codename,
  version, beta flag via `/etc/tontoo-release` plus env overrides),
  `os.fico` refresh at startup, `SETTINGS_OS_FICO` config key.

- 2026-09-07: Hardware backend. `hardware` module (CPU via `/proc/cpuinfo`,
  RAM size via `/proc/meminfo`, RAM type and modules via `dmidecode`,
  GPUs via DRM sysfs plus `lspci`), `sys.fico` refresh at startup,
  `SETTINGS_SYS_FICO` config key, `sdk`/`FishFile` dependency.
- 2026-09-07: Initial basis. `DaemonConfig`, `SettingsStore`, `LibraryManager`
  stub, socket accept stub, `lang/` files, `Headers/settings_daemon.h` stub.
