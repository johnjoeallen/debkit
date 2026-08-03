use std::fs;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::ownership::{OwnerProbe, OwnerResult, detect_owner};
use crate::engine::plan::ChangePlan;

/// Read-only, no config section: DHCP server ownership (competing-server detection is
/// the whole point — DebKit has no business deciding which DHCP server a host should
/// run) and which client backend is managing lease acquisition. Port-67 listeners are
/// the ground truth for "is a DHCP server actually running here" regardless of which
/// package it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DhcpObservation {
    server_owner: Option<String>,
    server_conflict: Option<Vec<String>>,
    port_67_listeners: Vec<String>,
    dhcp_client_backend: Option<String>,
    dnsmasq_has_dhcp_range: bool,
}

pub struct NetworkDhcp;

impl Module for NetworkDhcp {
    fn name(&self) -> &'static str {
        "network.dhcp"
    }

    fn discover(&self, _ctx: &Context) -> anyhow::Result<Observation> {
        let isc_dhcp_active = exec::systemctl_is_active("isc-dhcp-server");
        let dnsmasq_active = exec::systemctl_is_active("dnsmasq");
        let dnsmasq_has_dhcp_range = dnsmasq_has_dhcp_range();

        let owner_result = detect_owner(
            &[
                OwnerProbe::new("isc-dhcp-server", isc_dhcp_active),
                OwnerProbe::new("dnsmasq", dnsmasq_active && dnsmasq_has_dhcp_range),
            ],
            None,
        );

        let port_67_listeners = read_port_67_listeners();
        let dhcp_client_backend = detect_dhcp_client_backend();

        let data = DhcpObservation {
            server_owner: owner_result.owner().map(str::to_string),
            server_conflict: match &owner_result {
                OwnerResult::Conflict(owners) => Some(owners.clone()),
                _ => None,
            },
            port_67_listeners,
            dhcp_client_backend,
            dnsmasq_has_dhcp_range,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = data.server_owner.clone();
        if let Some(conflict) = &data.server_conflict {
            observation = observation.with_warning(format!(
                "multiple DHCP servers appear active: {}",
                conflict.join(", ")
            ));
        }
        if data.server_owner.is_none() && !data.port_67_listeners.is_empty() {
            observation = observation.with_warning(
                "something is bound to UDP/67 that wasn't attributed to a known DHCP server package"
                    .to_string(),
            );
        }
        Ok(observation)
    }

    fn diagnose(&self, _ctx: &Context, observation: &Observation) -> Diagnosis {
        let data: DhcpObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read network.dhcp observation: {err}"
                )]);
            }
        };

        match data.server_conflict {
            Some(conflict) => Diagnosis::conflict(conflict),
            None => Diagnosis::compliant(),
        }
    }

    fn plan(
        &self,
        _ctx: &Context,
        _observation: &Observation,
        _diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        // Diagnostics only — DebKit doesn't choose or configure a DHCP server.
        Ok(ChangePlan::new())
    }

    fn verify(&self, _ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let listeners = read_port_67_listeners();
        let isc_dhcp_active = exec::systemctl_is_active("isc-dhcp-server");
        let dnsmasq_active = exec::systemctl_is_active("dnsmasq") && dnsmasq_has_dhcp_range();

        Ok(vec![if isc_dhcp_active && dnsmasq_active {
            VerificationResult::fail(
                "at most one DHCP server is active",
                "both isc-dhcp-server and dnsmasq (with dhcp-range) are active",
            )
        } else if (isc_dhcp_active || dnsmasq_active) && listeners.is_empty() {
            VerificationResult::fail(
                "active DHCP server is actually listening",
                "a DHCP server service is active but nothing is bound to UDP/67",
            )
        } else {
            VerificationResult::pass("at most one DHCP server is active")
        }])
    }
}

fn read_port_67_listeners() -> Vec<String> {
    let Ok(raw) = exec::capture("ss", &["-uln"]) else {
        return Vec::new();
    };
    raw.lines()
        .filter(|line| {
            line.split_whitespace()
                .nth(4)
                .is_some_and(|addr| addr.ends_with(":67"))
        })
        .map(str::trim)
        .map(str::to_string)
        .collect()
}

fn detect_dhcp_client_backend() -> Option<String> {
    if exec::systemctl_is_active("NetworkManager") {
        return Some("NetworkManager".to_string());
    }
    if exec::systemctl_is_active("systemd-networkd") {
        return Some("systemd-networkd".to_string());
    }
    if exec::capture("pgrep", &["-x", "dhclient"]).is_ok() {
        return Some("dhclient".to_string());
    }
    None
}

fn dnsmasq_has_dhcp_range() -> bool {
    let paths = ["/etc/dnsmasq.conf"];
    for path in paths {
        if let Ok(raw) = fs::read_to_string(path)
            && raw
                .lines()
                .any(|line| line.trim_start().starts_with("dhcp-range"))
        {
            return true;
        }
    }
    let Ok(entries) = fs::read_dir("/etc/dnsmasq.d") else {
        return false;
    };
    entries.filter_map(|entry| entry.ok()).any(|entry| {
        fs::read_to_string(entry.path())
            .map(|raw| {
                raw.lines()
                    .any(|line| line.trim_start().starts_with("dhcp-range"))
            })
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(NetworkDhcp.name(), "network.dhcp");
    }

    fn config() -> crate::config::DebkitConfig {
        crate::config::DebkitConfig::default()
    }

    #[test]
    fn compliant_when_no_conflict() {
        let data = DhcpObservation {
            server_owner: Some("isc-dhcp-server".to_string()),
            server_conflict: None,
            port_67_listeners: vec!["UDP 0.0.0.0:67".to_string()],
            dhcp_client_backend: Some("NetworkManager".to_string()),
            dnsmasq_has_dhcp_range: false,
        };
        let config = config();
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = NetworkDhcp.diagnose(&ctx, &observation);
        assert!(diagnosis.compliant);
    }

    #[test]
    fn conflict_when_two_servers_active() {
        let data = DhcpObservation {
            server_owner: None,
            server_conflict: Some(vec!["isc-dhcp-server".to_string(), "dnsmasq".to_string()]),
            port_67_listeners: Vec::new(),
            dhcp_client_backend: None,
            dnsmasq_has_dhcp_range: true,
        };
        let config = config();
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = NetworkDhcp.diagnose(&ctx, &observation);
        assert!(diagnosis.conflict.is_some());
    }

    #[test]
    fn plan_is_always_empty() {
        let config = config();
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::json!({}));
        let diagnosis = Diagnosis::compliant();
        let plan = NetworkDhcp.plan(&ctx, &observation, &diagnosis).unwrap();
        assert!(plan.is_empty());
    }
}
