//! DNS backend owned by the settings daemon.
//!
//! Reads and writes the IPv4 DNS servers of the active NetworkManager
//! connection (`dns_get` public read, `dns_set` private write reserved
//! for the Settings app). An empty server list means DHCP (automatic).

use networkkit::util;
use serde::Serialize;

pub const OP_DNS_GET: &str = "dns_get";
pub const OP_DNS_SET: &str = "dns_set";

/// Default manual servers suggested by the Settings app.
pub const DEFAULT_DNS_SERVERS: &[&str] = &["1.1.1.1", "8.8.8.8"];

/// Effective DNS state: manual servers, or DHCP when `servers` is empty
/// and `manual` is false.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DnsState {
    pub servers: Vec<String>,
    pub manual: bool,
}

/// Parse user input into IPv4 servers. Accepts comma and/or whitespace
/// separated addresses, drops duplicates. Empty input means DHCP and
/// yields an empty vec. Returns Err on the first invalid address.
pub fn parse_dns_servers(input: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for part in input
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
    {
        if !is_ipv4(part) {
            return Err(format!("invalid IPv4 address: {}", part));
        }
        if !out.iter().any(|s| s == part) {
            out.push(part.to_string());
        }
    }
    Ok(out)
}

fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|p| {
        !p.is_empty()
            && p.len() <= 3
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u8>().is_ok()
    })
}

/// Name and UUID of the first active connection bound to a device.
fn active_connection() -> Result<(String, String), String> {
    let out = util::run(
        "nmcli",
        &[
            "-t",
            "-f",
            "NAME,UUID,DEVICE",
            "connection",
            "show",
            "--active",
        ],
    )
    .map_err(|e| e.to_string())?;
    for line in out.lines() {
        let fields = networkkit::wifi::split_terse(line);
        if fields.len() >= 3 && !fields[0].is_empty() && !fields[1].is_empty() && !fields[2].is_empty() {
            return Ok((fields[0].clone(), fields[1].clone()));
        }
    }
    Err("no active connection".to_string())
}

/// Read the effective DNS state of the active connection.
pub fn get() -> Result<DnsState, String> {
    let (_name, uuid) = active_connection()?;
    let out = util::run(
        "nmcli",
        &[
            "-t",
            "-f",
            "ipv4.dns,ipv4.ignore-auto-dns",
            "connection",
            "show",
            &uuid,
        ],
    )
    .map_err(|e| e.to_string())?;
    let mut servers: Vec<String> = Vec::new();
    let mut manual = false;
    for line in out.lines() {
        if let Some(value) = line.strip_prefix("ipv4.dns:") {
            servers = value
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        } else if let Some(value) = line.strip_prefix("ipv4.ignore-auto-dns:") {
            manual = value.trim() == "yes" || value.trim() == "1";
        }
    }
    Ok(DnsState { servers, manual })
}

/// Apply servers to the active connection and reactivate it so the
/// system resolver picks them up. Empty input clears the manual servers
/// (DHCP). Returns the effective state.
pub fn set_from_str(input: &str) -> Result<DnsState, String> {
    let servers = parse_dns_servers(input)?;
    set(&servers)
}

fn set(servers: &[String]) -> Result<DnsState, String> {
    let (_name, uuid) = active_connection()?;
    if servers.is_empty() {
        util::run(
            "nmcli",
            &[
                "connection",
                "modify",
                &uuid,
                "ipv4.dns",
                "",
                "ipv4.ignore-auto-dns",
                "no",
            ],
        )
        .map_err(|e| e.to_string())?;
    } else {
        let joined = servers.join(" ");
        util::run(
            "nmcli",
            &[
                "connection",
                "modify",
                &uuid,
                "ipv4.dns",
                &joined,
                "ipv4.ignore-auto-dns",
                "yes",
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    util::run("nmcli", &["connection", "up", &uuid]).map_err(|e| e.to_string())?;
    get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_and_multiple_servers() {
        assert_eq!(parse_dns_servers("1.1.1.1").unwrap(), vec!["1.1.1.1"]);
        assert_eq!(
            parse_dns_servers("1.1.1.1, 8.8.8.8").unwrap(),
            vec!["1.1.1.1", "8.8.8.8"]
        );
        assert_eq!(
            parse_dns_servers("  9.9.9.9\n1.1.1.1  ").unwrap(),
            vec!["9.9.9.9", "1.1.1.1"]
        );
    }

    #[test]
    fn empty_input_means_dhcp() {
        assert!(parse_dns_servers("").unwrap().is_empty());
        assert!(parse_dns_servers("  ,\n ").unwrap().is_empty());
    }

    #[test]
    fn drops_duplicates_keeping_order() {
        assert_eq!(
            parse_dns_servers("8.8.8.8, 1.1.1.1, 8.8.8.8").unwrap(),
            vec!["8.8.8.8", "1.1.1.1"]
        );
    }

    #[test]
    fn rejects_non_ipv4() {
        assert!(parse_dns_servers("example.com").is_err());
        assert!(parse_dns_servers("2001:4860:4860::8888").is_err());
        assert!(parse_dns_servers("999.1.1.1").is_err());
        assert!(parse_dns_servers("1.2.3").is_err());
        assert!(parse_dns_servers("1.2.3.4.5").is_err());
        assert!(parse_dns_servers("1.1.1.1, nope").is_err());
    }

    #[test]
    fn get_without_nmcli_errors() {
        if !networkkit::util::tool_available("nmcli") {
            assert!(get().is_err());
        }
    }

    #[test]
    fn set_without_nmcli_errors() {
        if !networkkit::util::tool_available("nmcli") {
            assert!(set_from_str("1.1.1.1, 8.8.8.8").is_err());
            assert!(set_from_str("").is_err());
        }
    }
}
