use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::ownership::{OwnerProbe, detect_owner};
use crate::engine::plan::{Change, ChangePlan, Risk};

const LINK_DIR: &str = "/etc/systemd/network";

/// Mostly read-only: interface inventory, active network-manager ownership, forwarding,
/// and rp_filter. The one declared-state piece is stable MAC-based interface naming via
/// systemd `.link` files (`network_interfaces.links` in config) — self-contained because
/// systemd-udevd applies it independent of which manager configures the interface
/// afterward, and it only takes effect on next boot/udev reload, never live. Full
/// role-based WAN/LAN configuration (addresses, forwarding, manager selection per
/// interface) is deferred to a later phase — that requires choosing/switching network
/// managers, real Phase-2-shaped complexity. This module's other job is answering "who
/// owns networking on this host" and catching the classic conflict where more than one
/// manager is fighting over the same interfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct InterfaceInfo {
    name: String,
    kind: String,
    mac_address: Option<String>,
    operstate: String,
    addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LinkState {
    mac: String,
    name: String,
    path: String,
    current_content: Option<String>,
    desired_content: String,
    /// Whether an interface named `name` currently exists — i.e. whether the rename has
    /// already taken effect, or is still pending a reboot/udev reload.
    already_renamed: bool,
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
    links: Vec<LinkState>,
}

pub struct NetworkInterfaces;

impl Module for NetworkInterfaces {
    fn name(&self) -> &'static str {
        "network.interfaces"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
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

        let links = ctx
            .config
            .network_interfaces
            .links
            .iter()
            .map(read_link_state)
            .collect();

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
            links,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = data.active_manager.clone();
        if let Some(conflict) = &data.owner_conflict {
            observation = observation.with_warning(format!(
                "multiple network managers appear active: {}",
                conflict.join(", ")
            ));
        }
        for link in &data.links {
            if link.current_content.is_none() {
                observation = observation.with_warning(format!(
                    "renaming to `{}` is declared but not yet applied — takes effect on next boot/udev reload",
                    link.name
                ));
            }
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
            return Diagnosis::conflict(conflict);
        }

        let findings: Vec<String> = data
            .links
            .iter()
            .filter(|link| link.current_content.as_deref() != Some(link.desired_content.as_str()))
            .map(|link| format!("{} does not match declared name `{}`", link.path, link.name))
            .collect();

        if findings.is_empty() {
            Diagnosis::compliant()
        } else {
            Diagnosis::mismatch(findings)
        }
    }

    fn plan(
        &self,
        _ctx: &Context,
        observation: &Observation,
        diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        let mut plan = ChangePlan::new();
        if diagnosis.compliant {
            return Ok(plan);
        }

        let data: NetworkInterfacesObservation = serde_json::from_value(observation.data.clone())?;
        for link in &data.links {
            if link.current_content.as_deref() != Some(link.desired_content.as_str()) {
                plan.push(
                    format!("rename `{}` to `{}` on next boot", link.mac, link.name),
                    Risk::Medium,
                    Change::WriteFile {
                        path: PathBuf::from(&link.path),
                        content: link.desired_content.clone(),
                    },
                );
            }
        }
        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
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

        for entry in &ctx.config.network_interfaces.links {
            let check_name = format!("interface at `{}` is named `{}`", entry.mac, entry.name);
            if Path::new("/sys/class/net").join(&entry.name).exists() {
                checks.push(VerificationResult::pass(check_name));
            } else {
                checks.push(VerificationResult::skipped(
                    check_name,
                    "not yet renamed — takes effect on next boot/udev reload",
                ));
            }
        }

        Ok(checks)
    }
}

fn link_path(name: &str) -> String {
    format!("{LINK_DIR}/10-debkit-{name}.link")
}

fn desired_link_content(mac: &str, name: &str) -> String {
    format!("# Managed by DebKit.\n[Match]\nMACAddress={mac}\n\n[Link]\nName={name}\n")
}

fn read_link_state(entry: &crate::config::LinkEntry) -> LinkState {
    let path = link_path(&entry.name);
    let current_content = fs::read_to_string(&path).ok();
    let already_renamed = Path::new("/sys/class/net").join(&entry.name).exists();
    LinkState {
        mac: entry.mac.clone(),
        name: entry.name.clone(),
        path,
        current_content,
        desired_content: desired_link_content(&entry.mac, &entry.name),
        already_renamed,
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
            links: Vec::new(),
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
            links: Vec::new(),
        };
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = NetworkInterfaces.diagnose(&ctx, &observation);
        assert!(diagnosis.conflict.is_some());
        // No links configured in this fixture, so still nothing to plan — but plan()
        // itself is no longer a permanent no-op now that link renaming exists.
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

    fn compliant_link_state() -> LinkState {
        let mac = "00:11:22:33:44:55".to_string();
        let name = "lan0".to_string();
        LinkState {
            path: link_path(&name),
            desired_content: desired_link_content(&mac, &name),
            current_content: Some(desired_link_content(&mac, &name)),
            mac,
            name,
            already_renamed: false,
        }
    }

    fn base_link_data(links: Vec<LinkState>) -> NetworkInterfacesObservation {
        NetworkInterfacesObservation {
            active_manager: Some("systemd-networkd".to_string()),
            owner_conflict: None,
            default_route_interface: Some("lan0".to_string()),
            ip_forward_v4: false,
            ip_forward_v6: false,
            rp_filter_all: None,
            rp_filter_default: None,
            interfaces: Vec::new(),
            links,
        }
    }

    #[test]
    fn desired_link_content_matches_systemd_link_format() {
        let content = desired_link_content("00:11:22:33:44:55", "lan0");
        assert!(content.contains("[Match]\nMACAddress=00:11:22:33:44:55"));
        assert!(content.contains("[Link]\nName=lan0"));
    }

    #[test]
    fn compliant_when_link_file_already_matches() {
        let data = base_link_data(vec![compliant_link_state()]);
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
    fn missing_link_file_produces_write_change() {
        let mut link = compliant_link_state();
        link.current_content = None;
        let data = base_link_data(vec![link]);
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = NetworkInterfaces.diagnose(&ctx, &observation);
        assert!(!diagnosis.compliant);
        let plan = NetworkInterfaces
            .plan(&ctx, &observation, &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 1);
        match &plan.changes[0].change {
            Change::WriteFile { path, .. } => {
                assert_eq!(
                    path,
                    &PathBuf::from("/etc/systemd/network/10-debkit-lan0.link")
                );
            }
            other => panic!("expected WriteFile, got {other:?}"),
        }
    }
}
