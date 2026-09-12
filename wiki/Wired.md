# Wired

The daemon owns wired network reporting. The `wired` backend lists
connected Ethernet interfaces with details for the Settings app info
menu (`wired_list` public read op). Device enumeration and addresses
come from NetworkManager; MAC, MTU, speed and driver come from sysfs.
Only connected interfaces are reported, ordered by interface name.

## Public read ops

| Op | Result |
|---|---|
| `wired_list` | `{"interfaces": [...]}` with `interface`, `connection`, `state`, `ipv4_addrs`, `gateway`, `mac`, `speed_mbps`, `mtu`, `driver` |

Without `nmcli` (or without a connected Ethernet interface list) the op
returns `ok: false` instead of partial data. Optional fields
(`gateway`, `speed_mbps`, `mtu`, `driver`) are `null` when unknown;
unknown link speed (`-1` in sysfs) maps to `null`.

## API

```rust
pub fn list() -> Result<Vec<WiredInfo>, String>
pub const OP_WIRED_LIST: &str = "wired_list";
```

- `list` is stateless; enumeration runs per call.
- The `parse_status_line`, `parse_device_show` and `read_sysfs` helpers
  are pure (sysfs base injectable) and unit-tested.
- Every function returns `Err(String)` on failure; the socket layer wraps
  it as `{"id": id, "ok": false, "error": "..."}`.

## Usage / Example

```bash
printf '{"id": 1, "op": "wired_list"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
```

```rust
use settings_daemon::wired;

let interfaces = wired::list()?;
for iface in &interfaces {
    println!("{} ({}) {}", iface.interface, iface.connection, iface.state);
}
```

## Cross References

- [Socket.md](Socket.md) -- protocol frames carrying these ops
- [Daemon.md](Daemon.md) -- configuration and runtime model
