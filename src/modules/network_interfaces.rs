use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::ownership::{OwnerProbe, detect_owner};
use crate::engine::plan::ChangePlan;

/// Read-only for now: interface inventory, active network-manager ownership, forwarding,
/// and rp_filter. There is no declared desired state yet (no config section) — stable
/// MAC-based naming and role-based WAN/LAN configuration (the doc's `network.interfaces`
/// write path) is deferred to a later phase. This module's whole value right now is
/// answering "who owns networking on this host" and catching the classic conflict where
/// more than one manager is fighting over the same interfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct InterfaceInfo {
    name: String,
    kind: String,
    mac_address: Option<String>,
    operstate: String,
    addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkInterfacesObservation {
    active_manager: Option<String>,
    owner_conflict: Option<Vec<String>>,
    default_route_interface: Option<String>,
    ip_forward_v4: bool,
    ip_forward_v6: bool,
    rp_filter_all: Option<String>,
    rp_filter_default: Option<String>,
    interfaces: Vec<InterfaceInfo>,
}

pub struct NetworkInterfaces;

impl Module for NetworkInterfaces {
    fn name(&self) -> &'static str {
        "network.interfaces"
    }

    fn discover(&self, _ctx: &Context) -> anyhow::Result<Observation> {
        let network_manager_active = exec::systemctl_is_active("NetworkManager");
        let networkd_active = exec::systemctl_is_active("systemd-networkd");
        let ifupdown_active = ifupdown_has_configured_interfaces();

        let owner_result = detect_owner(
            &[
                OwnerProbe::new("NetworkManager", network_manager_active),
                OwnerProbe::new("systemd-networkd", networkd_active),
                OwnerProbe::new("ifupdown", ifupdown_active),
            ],
            None,
        );

        let data = NetworkInterfacesObservation {
            active_manager: owner_result.owner().map(str::to_string),
            owner_conflict: match &owner_result {
                crate::engine::ownership::OwnerResult::Conflict(owners) => Some(owners.clone()),
                _ => None,
            },
            default_route_interface: default_route_interface(),
            ip_forward_v4: read_sysctl_bool("/proc/sys/net/ipv4/ip_forward"),
            ip_forward_v6: read_sysctl_bool("/proc/sys/net/ipv6/conf/all/forwarding"),
            rp_filter_all: read_trimmed("/proc/sys/net/ipv4/conf/all/rp_filter"),
            rp_filter_default: read_trimmed("/proc/sys/net/ipv4/conf/default/rp_filter"),
            interfaces: read_interfaces(),
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = data.active_manager.clone();
        if let Some(conflict) = &data.owner_conflict {
            observation = observation.with_warning(format!(
                "multiple network managers appear active: {}",
                conflict.join(", ")
            ));
        }
        Ok(observation)
    }

    fn diagnose(&self, _ctx: &Context, observation: &Observation) -> Diagnosis {
        let data: NetworkInterfacesObservation =
            match serde_json::from_value(observation.data.clone()) {
                Ok(data) => data,
                Err(err) => {
                    return Diagnosis::mismatch(vec![format!(
                        "failed to read network.interfaces observation: {err}"
                    )]);
                }
            };

        if let Some(conflict) = data.owner_conflict {
            Diagnosis::conflict(conflict)
        } else {
            Diagnosis::compliant()
        }
    }

    fn plan(
        &self,
        _ctx: &Context,
        _observation: &Observation,
        _diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        // No declared desired state yet — see module doc comment.
        Ok(ChangePlan::new())
    }

    fn verify(&self, _ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let mut checks = Vec::new();

        match default_route_interface() {
            Some(iface) => checks.push(VerificationResult::pass(format!(
                "default route present via `{iface}`"
            ))),
            None => checks.push(VerificationResult::fail(
                "default route present",
                "no default route found",
            )),
        }

        let network_manager_active = exec::systemctl_is_active("NetworkManager");
        let networkd_active = exec::systemctl_is_active("systemd-networkd");
        if network_manager_active && networkd_active {
            checks.push(VerificationResult::fail(
                "exactly one network manager is active",
                "both NetworkManager and systemd-networkd are active",
            ));
        } else {
            checks.push(VerificationResult::pass(
                "exactly one network manager is active",
            ));
        }

        Ok(checks)
    }
}

fn read_interfaces() -> Vec<InterfaceInfo> {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let mut interfaces = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "lo" {
                return None;
            }
            let path = entry.path();
            let kind = if path.join("bridge").exists() {
                "bridge"
            } else if path.join("wireless").exists() {
                "wireless"
            } else if path.join("device").exists() {
                "wired"
            } else {
                "virtual"
            };
            let mac_address = fs::read_to_string(path.join("address"))
                .ok()
                .map(|raw| raw.trim().to_string());
            let operstate = fs::read_to_string(path.join("operstate"))
                .ok()
                .map(|raw| raw.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let addresses = read_addresses(&name);
            Some(InterfaceInfo {
                name,
                kind: kind.to_string(),
                mac_address,
                operstate,
                addresses,
            })
        })
        .collect::<Vec<_>>();
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

fn read_addresses(iface: &str) -> Vec<String> {
    let Ok(raw) = exec::capture("ip", &["-br", "addr", "show", "dev", iface]) else {
        return Vec::new();
    };
    raw.lines()
        .flat_map(|line| line.split_whitespace().skip(2))
        .map(str::to_string)
        .collect()
}

fn default_route_interface() -> Option<String> {
    let raw = exec::capture("ip", &["route", "show", "default"]).ok()?;
    raw.lines().find_map(|line| {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        parts
            .windows(2)
            .find(|pair| pair[0] == "dev")
            .map(|pair| pair[1].to_string())
    })
}

fn ifupdown_has_configured_interfaces() -> bool {
    let Ok(raw) = fs::read_to_string("/etc/network/interfaces") else {
        return false;
    };
    raw.lines()
        .map(str::trim)
        .any(|line| line.starts_with("iface") && !line.contains(" lo "))
}

fn read_sysctl_bool(path: &str) -> bool {
    read_trimmed(path).as_deref() == Some("1")
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(Path::new(path))
        .ok()
        .map(|raw| raw.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(NetworkInterfaces.name(), "network.interfaces");
    }

    fn config() -> crate::config::DebkitConfig {
        crate::config::DebkitConfig::default()
    }

    #[test]
    fn compliant_when_no_conflict() {
        let data = NetworkInterfacesObservation {
            active_manager: Some("NetworkManager".to_string()),
            owner_conflict: None,
            default_route_interface: Some("enp5s0".to_string()),
            ip_forward_v4: false,
            ip_forward_v6: false,
            rp_filter_all: Some("2".to_string()),
            rp_filter_default: Some("2".to_string()),
            interfaces: Vec::new(),
        };
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = NetworkInterfaces.diagnose(&ctx, &observation);
        assert!(diagnosis.compliant);
        let plan = NetworkInterfaces
            .plan(&ctx, &observation, &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn conflict_when_two_managers_active() {
        let data = NetworkInterfacesObservation {
            active_manager: None,
            owner_conflict: Some(vec![
                "NetworkManager".to_string(),
                "systemd-networkd".to_string(),
            ]),
            default_route_interface: Some("enp5s0".to_string()),
            ip_forward_v4: false,
            ip_forward_v6: false,
            rp_filter_all: None,
            rp_filter_default: None,
            interfaces: Vec::new(),
        };
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = NetworkInterfaces.diagnose(&ctx, &observation);
        assert!(diagnosis.conflict.is_some());
        // Still no plan: this module never touches the system yet.
        let plan = NetworkInterfaces
            .plan(&ctx, &observation, &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn ifupdown_detection_ignores_loopback_stanza() {
        let raw = "auto lo\niface lo inet loopback\n";
        assert!(
            !raw.lines()
                .map(str::trim)
                .any(|line| line.starts_with("iface") && !line.contains(" lo "))
        );
    }
}
