# Library

`LibraryManager` is the registry for settings libraries. Basis only: it maps
names to versions in memory so the daemon, store and socket already share one
owner. Discovery, validation, dynamic loading and version negotiation come
later.

## API

### Types

```rust
pub struct LibraryEntry {
  pub name: String,
  pub version: String,
}
```

| Field | Type | Description |
|---|---|---|
| `name` | `String` | Library name, e.g. `"appearance"` |
| `version` | `String` | Library version, e.g. `"0.1.0"` |

### Constructors

```rust
impl LibraryManager {
  pub fn new() -> Self;
}
```

Creates an empty registry.

### Functions

```rust
impl LibraryManager {
  pub fn register(&mut self, name: &str, version: &str);
  pub fn unregister(&mut self, name: &str) -> bool;
  pub fn is_registered(&self, name: &str) -> bool;
  pub fn list(&self) -> Vec<LibraryEntry>;
}
```

- `register` inserts or replaces the entry for `name`.
- `unregister` returns `true` when an entry was removed.
- `is_registered` returns `true` when `name` has an entry.
- `list` returns entries sorted by name.

## Usage / Example

```rust
use settings_daemon::LibraryManager;

let mut libs = LibraryManager::new();
libs.register("appearance", "0.1.0");
assert!(libs.is_registered("appearance"));
```

## Cross References

- [Daemon.md](Daemon.md) – manager lifetime inside the daemon
- [Socket.md](Socket.md) – future socket-triggered reload
