# Dns

The daemon owns DNS control. The `dns` backend reads and writes the IPv4
DNS servers of the active NetworkManager connection. `dns_get` is a
public read op, `dns_set` is a private write op reserved for the
Settings app (`com.tontoo.systemsettings`). An empty server list means
DHCP (automatic).

## Public read ops

| Op | Result |
|---|---|
| `dns_get` | `{"servers": [...], "manual": bool}` |

`servers` holds the manual IPv4 servers (empty for DHCP). `manual`
reflects `ipv4.ignore-auto-dns`. Without an active connection the op
returns `ok: false` instead of partial data.

## Private write ops

| Op | Params | Result |
|---|---|---|
| `dns_set` | `servers` (comma/whitespace separated IPv4, empty for DHCP) | The effective `DnsState` |

Rules:

- `dns_set` validates every address (dotted quad, strict) and rejects
  the whole input on the first invalid address, never a partial result.
- The servers are applied with `nmcli connection modify <uuid>
  ipv4.dns ... ipv4.ignore-auto-dns yes|no` and the connection is
  reactivated (`nmcli connection up`) so the system resolver picks them
  up immediately.
- Empty input clears the manual servers back to DHCP.
- The Settings app suggests `1.1.1.1, 8.8.8.8` as the default manual
  servers (see `DEFAULT_DNS_SERVERS`).

## API

```rust
pub fn get() -> Result<DnsState, String>
pub fn set_from_str(input: &str) -> Result<DnsState, String>
pub fn parse_dns_servers(input: &str) -> Result<Vec<String>, String>
pub const DEFAULT_DNS_SERVERS: &[&str]
```

- `parse_dns_servers` is pure and unit-tested (separators, dedup,
  DHCP empty, strict IPv4 rejection).
- Every function returns `Err(String)` on failure; the socket layer wraps
  it as `{"id": id, "ok": false, "error": "..."}`.

```rust
pub const OP_DNS_GET: &str = "dns_get";
pub const OP_DNS_SET: &str = "dns_set";
```

## Usage / Example

```bash
printf '{"id": 1, "op": "dns_get"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 2, "op": "dns_set", "params": {"servers": "1.1.1.1, 8.8.8.8"}}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
```

```rust
use settings_daemon::dns;

let state = dns::get()?;
for server in &state.servers {
    println!("{} (manual: {})", server, state.manual);
}
```

## Cross References

- [Socket.md](Socket.md) -- protocol frames carrying these ops
- [Daemon.md](Daemon.md) -- configuration and runtime model
