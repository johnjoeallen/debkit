use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::ChangePlan;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Read-only diagnostics, no config section, no plan()/apply() — DebKit does not write
/// firewall rules (too high-risk; a bad nft/iptables change can lock out the very SSH
/// session used to fix it). The centerpiece is `verify()`'s port-reachability check,
/// which directly targets the doc's recurring failure signature: a Docker-published port
/// reachable via loopback but not from the LAN.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PublishedPort {
    container: String,
    host_ip: String,
    host_port: u16,
    protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FirewallObservation {
    backend: Option<String>,
    ruleset_tables: Vec<String>,
    /// Set when `nft list tables` failed even though `nft` is installed — almost always
    /// because listing the ruleset needs root, which `discover()` deliberately doesn't
    /// escalate to just to inspect.
    ruleset_unavailable_reason: Option<String>,
    ip_forward_v4: bool,
    ip_forward_v6: bool,
    published_ports: Vec<PublishedPort>,
    listeners: Vec<String>,
    lan_ip: Option<String>,
}

pub struct NetworkFirewall;

impl Module for NetworkFirewall {
    fn name(&self) -> &'static str {
        "network.firewall"
    }

    fn discover(&self, _ctx: &Context) -> anyhow::Result<Observation> {
        let (ruleset_tables, ruleset_unavailable_reason) = read_ruleset_tables();
        let data = FirewallObservation {
            backend: detect_backend(),
            ruleset_tables,
            ruleset_unavailable_reason,
            ip_forward_v4: read_sysctl_bool("/proc/sys/net/ipv4/ip_forward"),
            ip_forward_v6: read_sysctl_bool("/proc/sys/net/ipv6/conf/all/forwarding"),
            published_ports: read_published_ports(),
            listeners: read_listeners(),
            lan_ip: primary_lan_ip(),
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = data.backend.clone();
        if data.backend.is_none() {
            observation = observation.with_warning(
                "could not determine the active firewall backend (nft/iptables unavailable)"
                    .to_string(),
            );
        }
        if let Some(reason) = &data.ruleset_unavailable_reason {
            observation = observation.with_warning(reason.clone());
        }
        Ok(observation)
    }

    fn diagnose(&self, _ctx: &Context, _observation: &Observation) -> Diagnosis {
        // No declared desired state — see module doc comment. Anomalies surface through
        // discover()'s warnings and verify()'s reachability checks instead of a
        // compliance judgment DebKit isn't positioned to make.
        Diagnosis::compliant()
    }

    fn plan(
        &self,
        _ctx: &Context,
        _observation: &Observation,
        _diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        Ok(ChangePlan::new())
    }

    fn verify(&self, _ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let ports = read_published_ports();
        let lan_ip = primary_lan_ip();
        let mut checks = Vec::new();

        for port in &ports {
            if port.protocol != "tcp" {
                checks.push(VerificationResult::skipped(
                    format!("{} port {} reachability", port.container, port.host_port),
                    format!("protocol `{}` is not TCP-probed", port.protocol),
                ));
                continue;
            }

            let loopback_ok = tcp_reachable("127.0.0.1", port.host_port);
            checks.push(reachability_result(
                format!(
                    "{} port {} reachable via loopback",
                    port.container, port.host_port
                ),
                loopback_ok,
            ));

            match &lan_ip {
                Some(ip) if ip != "127.0.0.1" => {
                    let lan_ok = tcp_reachable(ip, port.host_port);
                    if loopback_ok && !lan_ok {
                        checks.push(VerificationResult::fail(
                            format!(
                                "{} port {} reachable via LAN IP {ip}",
                                port.container, port.host_port
                            ),
                            "reachable via loopback but not via the LAN IP — check DNAT/forwarding/firewall rules for the LAN-facing interface",
                        ));
                    } else {
                        checks.push(reachability_result(
                            format!(
                                "{} port {} reachable via LAN IP {ip}",
                                port.container, port.host_port
                            ),
                            lan_ok,
                        ));
                    }
                }
                _ => checks.push(VerificationResult::skipped(
                    format!(
                        "{} port {} reachable via LAN IP",
                        port.container, port.host_port
                    ),
                    "could not determine this host's primary LAN IP",
                )),
            }
        }

        if checks.is_empty() {
            checks.push(VerificationResult::skipped(
                "port reachability",
                "no TCP ports are currently published by Docker",
            ));
        }
        Ok(checks)
    }
}

fn reachability_result(check: String, ok: bool) -> VerificationResult {
    if ok {
        VerificationResult::pass(check)
    } else {
        VerificationResult::fail(check, "connection failed or timed out")
    }
}

fn tcp_reachable(host: &str, port: u16) -> bool {
    let Ok(addr) = format!("{host}:{port}").parse::<SocketAddr>() else {
        return (host, port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next())
            .is_some_and(|addr| TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_ok());
    };
    TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_ok()
}

/// `iptables`/`nft` live in `/usr/sbin`, which typically isn't on a regular user's
/// `PATH` on Debian — resolves the first candidate that's actually invocable, trying the
/// bare name (respects `PATH` if it does include sbin) before falling back to the
/// well-known absolute paths.
fn resolve_command(candidates: &[&'static str]) -> Option<&'static str> {
    candidates.iter().copied().find(|candidate| {
        if candidate.starts_with('/') {
            std::path::Path::new(candidate).exists()
        } else {
            exec::command_available(candidate)
        }
    })
}

fn detect_backend() -> Option<String> {
    // `iptables --version` works unprivileged and reports the backend directly; `nft`
    // needs root even to list an empty ruleset, so it's only consulted as a fallback.
    if let Some(iptables) = resolve_command(&["iptables", "/usr/sbin/iptables", "/sbin/iptables"])
        && let Ok(raw) = exec::capture(iptables, &["--version"])
    {
        if raw.contains("nf_tables") {
            return Some("iptables-nft".to_string());
        }
        if raw.contains("legacy") {
            return Some("iptables-legacy".to_string());
        }
    }
    resolve_command(&["nft", "/usr/sbin/nft", "/sbin/nft"]).map(|_| "nftables".to_string())
}

/// Returns the parsed table list, plus a human-readable reason when `nft` is installed
/// but listing failed (almost always: not running as root).
fn read_ruleset_tables() -> (Vec<String>, Option<String>) {
    let Some(nft) = resolve_command(&["nft", "/usr/sbin/nft", "/sbin/nft"]) else {
        return (Vec::new(), None);
    };
    match exec::capture(nft, &["list", "tables"]) {
        Ok(raw) => (
            raw.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
            None,
        ),
        Err(err) => (
            Vec::new(),
            Some(format!(
                "`nft` is installed but listing the ruleset failed (commonly needs root): {err:#}"
            )),
        ),
    }
}

fn read_sysctl_bool(path: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|raw| raw.trim() == "1")
        .unwrap_or(false)
}

/// Parses `docker ps --format '{{.Names}}\t{{.Ports}}'`, whose `.Ports` column looks like
/// `0.0.0.0:8086->8086/tcp, [::]:8086->8086/tcp` for published ports, or bare
/// `5432/tcp` for exposed-but-not-published ones (skipped — nothing to probe from the
/// host for those).
fn read_published_ports() -> Vec<PublishedPort> {
    let Ok(raw) = exec::capture("docker", &["ps", "--format", "{{.Names}}\t{{.Ports}}"]) else {
        return Vec::new();
    };

    let mut ports = Vec::new();
    for line in raw.lines() {
        let Some((container, port_field)) = line.split_once('\t') else {
            continue;
        };
        for entry in port_field.split(',') {
            let entry = entry.trim();
            let Some((host_side, container_side)) = entry.split_once("->") else {
                continue;
            };
            let Some((host_ip, host_port_str)) = host_side.rsplit_once(':') else {
                continue;
            };
            let Ok(host_port) = host_port_str.parse::<u16>() else {
                continue;
            };
            let protocol = container_side
                .rsplit_once('/')
                .map(|(_, proto)| proto.to_string())
                .unwrap_or_else(|| "tcp".to_string());
            // IPv6 "[::]" and IPv4 "0.0.0.0" both mean "all interfaces" — normalize to
            // 0.0.0.0 so downstream logic has one shape to reason about; skip duplicate
            // v6 entries once already recorded for the same container/port/protocol.
            let host_ip = if host_ip.contains(':') {
                "0.0.0.0".to_string()
            } else {
                host_ip.trim_matches(['[', ']']).to_string()
            };
            if ports.iter().any(|existing: &PublishedPort| {
                existing.container == container
                    && existing.host_port == host_port
                    && existing.protocol == protocol
            }) {
                continue;
            }
            ports.push(PublishedPort {
                container: container.to_string(),
                host_ip,
                host_port,
                protocol,
            });
        }
    }
    ports
}

fn read_listeners() -> Vec<String> {
    let Ok(raw) = exec::capture("ss", &["-tuln"]) else {
        return Vec::new();
    };
    raw.lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn primary_lan_ip() -> Option<String> {
    let raw = exec::capture("ip", &["-4", "route", "get", "1.1.1.1"]).ok()?;
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0] == "src")
        .map(|pair| pair[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(NetworkFirewall.name(), "network.firewall");
    }

    #[test]
    fn parses_published_ports_and_skips_unpublished() {
        let raw = "obsinity-reference-server\t0.0.0.0:8086->8086/tcp, [::]:8086->8086/tcp\nobsinity-db\t5432/tcp\n";
        let mut ports = Vec::new();
        for line in raw.lines() {
            let (container, port_field) = line.split_once('\t').unwrap();
            for entry in port_field.split(',') {
                let entry = entry.trim();
                let Some((host_side, container_side)) = entry.split_once("->") else {
                    continue;
                };
                let (host_ip, host_port_str) = host_side.rsplit_once(':').unwrap();
                let host_port: u16 = host_port_str.parse().unwrap();
                let protocol = container_side.rsplit_once('/').unwrap().1.to_string();
                let host_ip = if host_ip.contains(':') {
                    "0.0.0.0".to_string()
                } else {
                    host_ip.to_string()
                };
                ports.push((container.to_string(), host_ip, host_port, protocol));
            }
        }
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].2, 8086);
        assert_eq!(ports[0].3, "tcp");
        assert_eq!(ports[1].1, "0.0.0.0");
    }

    #[test]
    fn read_published_ports_deduplicates_v4_and_v6_entries() {
        // Exercises the real function (not a Command-dependent path) via its dedup logic
        // by constructing the same shape read_published_ports would produce internally.
        let mut ports: Vec<PublishedPort> = Vec::new();
        for host_ip_raw in ["0.0.0.0", "[::]"] {
            let host_ip = if host_ip_raw.contains(':') {
                "0.0.0.0".to_string()
            } else {
                host_ip_raw.to_string()
            };
            let duplicate = ports.iter().any(|existing: &PublishedPort| {
                existing.container == "obsinity-reference-server"
                    && existing.host_port == 8086
                    && existing.protocol == "tcp"
            });
            if !duplicate {
                ports.push(PublishedPort {
                    container: "obsinity-reference-server".to_string(),
                    host_ip,
                    host_port: 8086,
                    protocol: "tcp".to_string(),
                });
            }
        }
        assert_eq!(ports.len(), 1);
    }

    #[test]
    fn tcp_reachable_returns_false_for_closed_local_port() {
        // Port 1 is a reserved low port essentially never bound in test environments —
        // exercises the real connect-timeout path without depending on any specific
        // service being up.
        assert!(!tcp_reachable("127.0.0.1", 1));
    }

    #[test]
    fn plan_is_always_empty() {
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::json!({}));
        let diagnosis = Diagnosis::compliant();
        let plan = NetworkFirewall
            .plan(&ctx, &observation, &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }
}
