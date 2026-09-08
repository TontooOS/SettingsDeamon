# Os

The `os` module is the backend service for OS identity. It writes the
`os.fico` file (Fish Config format) on every daemon start with the display
name, codename, version and beta-channel flag. There is no query API yet on
purpose: socket access and the client library are roadmap items.

## Sources

Override chain, first hit wins per field:

| Priority | Source | Note |
|---|---|---|
| 1 | `/etc/tontoo-release` | `KEY=value` lines, shipped by BaseOS |
| 2 | `TONTOO_OS_*` environment | Intended for tests |
| 3 | Compiled defaults | `TontooOS Seal 26.1.0`, `beta=false` |

New releases only need a new `/etc/tontoo-release` file, never a daemon
rebuild.

## API

### Types

```rust
pub struct OsInfo {
  pub name: String,
  pub display_name: String,
  pub codename: String,
  pub version: String,
  pub beta: bool,
}
```

| Field | Type | Current value | Description |
|---|---|---|---|
| `name` | `String` | `TontooOS` | OS family name |
| `display_name` | `String` | `TontooOS Seal` | Name shown in UI |
| `codename` | `String` | `Seal` | Release codename |
| `version` | `String` | `26.1.0` | Release version |
| `beta` | `bool` | `false` | True when the beta channel is activated |

### Functions

```rust
pub fn collect() -> OsInfo;
```

Collects OS facts through the override chain. Never fails.

```rust
pub fn write_os_fico(path: &Path) -> io::Result<OsInfo>;
```

Collects and writes the file, creating parent directories. Returns the
snapshot that was written. Returns `Err` only on real I/O errors.

## File Format

```text
os {
    name: TontooOS
    display_name: "TontooOS Seal"
    codename: Seal
    version: "26.1.0"
    beta: false
}
```

Release file example (`/etc/tontoo-release`):

```bash
NAME=TontooOS
DISPLAY_NAME="TontooOS Seal"
CODENAME=Seal
VERSION=26.1.0
BETA=false
```

## Usage / Example

```rust
use settings_daemon::os;
use std::path::Path;

// Daemon startup: refreshed every start.
let info = os::write_os_fico(Path::new("/Library/Preferences/SystemConfiguration/os.fico"))?;
println!("{} {} beta={}", info.display_name, info.version, info.beta);
```

## Cross References

- [Daemon.md](Daemon.md) – startup refresh wiring
- [Hardware.md](Hardware.md) – sibling snapshot in `sys.fico`
- [Socket.md](Socket.md) – future query dispatch for these facts
