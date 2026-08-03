use std::path::PathBuf;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::ownership::{OwnerProbe, OwnerResult, detect_owner};
use crate::engine::plan::{Change, ChangePlan, Risk, ServiceActionKind};
use crate::install::wake_on_lan as legacy;

/// The observed state needed to diagnose/plan without touching the system again — everything
/// `diagnose()`/`plan()` need is captured here by `discover()` so neither has to re-run
/// `collect_report`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WakeOnLanObservation {
    hostname: String,
    requested_backend: String,
    selected_backend: String,
    network_manager_installed: bool,
    network_manager_running: bool,
    ethtool_installed: bool,
    selected_interfaces: Vec<String>,
    selection_error: Option<String>,
    primary_interface: Option<PrimaryInterfaceState>,
    owner_conflict: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PrimaryInterfaceState {
    name: String,
    mac_address: Option<String>,
    nm_connection: Option<String>,
    nm_wake_on_lan: Option<String>,
    ethtool_wake_on: Option<String>,
}

pub struct NetworkWakeOnLan;

impl Module for NetworkWakeOnLan {
    fn name(&self) -> &'static str {
        "network.wake_on_lan"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let config = &ctx.config.wake_on_lan;
        let report = legacy::collect_report(ctx.config)?;
        let request = legacy::parse_backend(&config.backend)?;

        let (selected, selection_error) = match legacy::selected_interfaces(&report, config) {
            Ok(list) => (list, None),
            Err(err) => (Vec::new(), Some(err.to_string())),
        };
        let selected_backend = legacy::select_backend(&report, &selected, request);

        let primary_interface = selected.first().and_then(|name| {
            legacy::interface_by_name(&report, name)
                .ok()
                .map(|iface| PrimaryInterfaceState {
                    name: iface.name.clone(),
                    mac_address: iface.mac_address.clone(),
                    nm_connection: iface.nm_connection.clone(),
                    nm_wake_on_lan: iface.nm_wake_on_lan.clone(),
                    ethtool_wake_on: iface.ethtool_wake_on.clone(),
                })
        });

        // Ownership is about who is *currently* enforcing wake-on-lan on the target
        // interface, independent of which backend the config would select going
        // forward — both can be true at once, which is exactly the conflict doc §4.1
        // says DebKit must refuse to plan against.
        let nm_active = primary_interface
            .as_ref()
            .map(|iface| iface.nm_wake_on_lan.as_deref() == Some("magic"))
            .unwrap_or(false);
        let ethtool_active = primary_interface
            .as_ref()
            .map(|iface| exec::systemctl_is_enabled(&format!("debkit-wol@{}.service", iface.name)))
            .unwrap_or(false);
        let override_owner = match config.backend.as_str() {
            "network_manager" | "networkmanager" => Some("NetworkManager"),
            "ethtool" => Some("ethtool"),
            _ => None,
        };
        let owner_result = detect_owner(
            &[
                OwnerProbe::new("NetworkManager", nm_active),
                OwnerProbe::new("ethtool", ethtool_active),
            ],
            override_owner,
        );

        let data = WakeOnLanObservation {
            hostname: report.hostname.clone(),
            requested_backend: request.as_str().to_string(),
            selected_backend: selected_backend.as_str().to_string(),
            network_manager_installed: report.network_manager_installed,
            network_manager_running: report.network_manager_running,
            ethtool_installed: report.ethtool_installed,
            selected_interfaces: selected,
            selection_error: selection_error.clone(),
            primary_interface,
            owner_conflict: match &owner_result {
                OwnerResult::Conflict(owners) => Some(owners.clone()),
                _ => None,
            },
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = owner_result.owner().map(str::to_string);
        for warning in &report.warnings {
            observation = observation.with_warning(warning.clone());
        }
        if let Some(err) = &selection_error {
            observation = observation.with_warning(format!("interface selection failed: {err}"));
        }
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.wake_on_lan.enabled {
            return Diagnosis::compliant();
        }

        let data: WakeOnLanObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read network.wake_on_lan observation: {err}"
                )]);
            }
        };

        if let Some(conflict) = data.owner_conflict {
            return Diagnosis::conflict(conflict);
        }
        if let Some(err) = data.selection_error {
            return Diagnosis::mismatch(vec![format!("cannot select target interface: {err}")]);
        }
        let Some(primary) = data.primary_interface else {
            return Diagnosis::mismatch(vec![
                "no wake-on-lan target interface resolved".to_string(),
            ]);
        };

        if data.selected_backend == "network_manager" {
            if primary.nm_wake_on_lan.as_deref() == Some("magic") {
                Diagnosis::compliant()
            } else {
                Diagnosis::mismatch(vec![format!(
                    "NetworkManager connection `{}` is not configured for wake-on-lan magic",
                    primary.nm_connection.as_deref().unwrap_or("<none>")
                )])
            }
        } else if primary.ethtool_wake_on.as_deref() == Some("g") {
            Diagnosis::compliant()
        } else {
            Diagnosis::mismatch(vec![format!(
                "ethtool wake-on-lan for `{}` is not set to magic (`g`)",
                primary.name
            )])
        }
    }

    fn plan(
        &self,
        ctx: &Context,
        observation: &Observation,
        diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        let mut plan = ChangePlan::new();
        if !ctx.config.wake_on_lan.enabled || diagnosis.compliant {
            return Ok(plan);
        }
        if let Some(conflict) = &diagnosis.conflict {
            anyhow::bail!(
                "refusing to plan network.wake_on_lan: competing owners are active ({}); set `wake_on_lan.backend` explicitly to resolve",
                conflict.join(", ")
            );
        }

        let data: WakeOnLanObservation = serde_json::from_value(observation.data.clone())
            .context("failed to read network.wake_on_lan observation")?;
        let primary = data.primary_interface.context(
            data.selection_error
                .unwrap_or_else(|| "no wake-on-lan target interface resolved".to_string()),
        )?;

        if data.selected_backend == "network_manager" {
            let connection = primary.nm_connection.as_deref().with_context(|| {
                format!(
                    "NetworkManager has no active connection for `{}`",
                    primary.name
                )
            })?;
            plan.push(
                format!("configure NetworkManager wake-on-lan magic for `{connection}`"),
                Risk::Low,
                Change::RunCommand {
                    program: "nmcli".to_string(),
                    args: vec![
                        "connection".to_string(),
                        "modify".to_string(),
                        connection.to_string(),
                        "802-3-ethernet.wake-on-lan".to_string(),
                        "magic".to_string(),
                    ],
                },
            );
        } else {
            plan.push(
                format!(
                    "enable runtime ethtool wake-on-lan magic for `{}`",
                    primary.name
                ),
                Risk::Low,
                Change::RunCommand {
                    program: "ethtool".to_string(),
                    args: vec![
                        "-s".to_string(),
                        primary.name.clone(),
                        "wol".to_string(),
                        "g".to_string(),
                    ],
                },
            );
            plan.push(
                "install debkit-wol@ systemd template unit",
                Risk::Medium,
                Change::WriteFile {
                    path: PathBuf::from(legacy::ETHTOOL_SERVICE_PATH),
                    content: legacy::ETHTOOL_SERVICE.to_string(),
                },
            );
            plan.push(
                format!("enable debkit-wol@{}.service for persistence", primary.name),
                Risk::Low,
                Change::ServiceAction {
                    unit: format!("debkit-wol@{}.service", primary.name),
                    action: ServiceActionKind::EnableNow,
                },
            );
        }

        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let config = &ctx.config.wake_on_lan;
        if !config.enabled {
            return Ok(vec![VerificationResult::skipped(
                "wake-on-lan",
                "disabled in config",
            )]);
        }

        let report = legacy::collect_report(ctx.config)?;
        let selected = legacy::selected_interfaces(&report, config)?;
        let Some(name) = selected.first() else {
            return Ok(vec![VerificationResult::fail(
                "wake-on-lan target",
                "no interface selected",
            )]);
        };
        let iface = legacy::interface_by_name(&report, name)?;

        let mut checks = Vec::new();
        if report.network_manager_installed && iface.nm_wake_on_lan.is_some() {
            checks.push(if iface.nm_wake_on_lan.as_deref() == Some("magic") {
                VerificationResult::pass(format!("NetworkManager wake-on-lan for {name}"))
            } else {
                VerificationResult::fail(
                    format!("NetworkManager wake-on-lan for {name}"),
                    format!(
                        "reported `{}`",
                        iface.nm_wake_on_lan.as_deref().unwrap_or("unknown")
                    ),
                )
            });
        }
        if report.ethtool_installed {
            checks.push(match iface.ethtool_wake_on.as_deref() {
                Some("g") => VerificationResult::pass(format!("ethtool Wake-on for {name}")),
                Some(state) => VerificationResult::fail(
                    format!("ethtool Wake-on for {name}"),
                    format!("reported `Wake-on: {state}`"),
                ),
                None => VerificationResult::skipped(
                    format!("ethtool Wake-on for {name}"),
                    "ethtool did not report a value",
                ),
            });
        }
        if checks.is_empty() {
            checks.push(VerificationResult::skipped(
                "wake-on-lan",
                "neither NetworkManager nor ethtool reported wake-on-lan state",
            ));
        }
        Ok(checks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(NetworkWakeOnLan.name(), "network.wake_on_lan");
    }

    fn observation(data: WakeOnLanObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn base_data() -> WakeOnLanObservation {
        WakeOnLanObservation {
            hostname: "spitfire".to_string(),
            requested_backend: "auto".to_string(),
            selected_backend: "network_manager".to_string(),
            network_manager_installed: true,
            network_manager_running: true,
            ethtool_installed: false,
            selected_interfaces: vec!["enp1s0".to_string()],
            selection_error: None,
            primary_interface: Some(PrimaryInterfaceState {
                name: "enp1s0".to_string(),
                mac_address: Some("aa:bb:cc:dd:ee:ff".to_string()),
                nm_connection: Some("Wired connection 1".to_string()),
                nm_wake_on_lan: Some("magic".to_string()),
                ethtool_wake_on: None,
            }),
            owner_conflict: None,
        }
    }

    fn config() -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.wake_on_lan.enabled = true;
        config
    }

    #[test]
    fn compliant_when_nm_already_set_to_magic() {
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = NetworkWakeOnLan.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
        let plan = NetworkWakeOnLan
            .plan(&ctx, &observation(base_data()), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn mismatch_produces_nmcli_change_when_not_yet_magic() {
        let mut data = base_data();
        data.primary_interface.as_mut().unwrap().nm_wake_on_lan = Some("d".to_string());
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = NetworkWakeOnLan.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = NetworkWakeOnLan
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 1);
        match &plan.changes[0].change {
            Change::RunCommand { program, args } => {
                assert_eq!(program, "nmcli");
                assert!(args.contains(&"magic".to_string()));
            }
            other => panic!("expected RunCommand, got {other:?}"),
        }
    }

    #[test]
    fn ethtool_backend_produces_three_changes() {
        let mut data = base_data();
        data.selected_backend = "ethtool".to_string();
        data.primary_interface.as_mut().unwrap().ethtool_wake_on = Some("d".to_string());
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = NetworkWakeOnLan.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = NetworkWakeOnLan
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 3);
    }

    #[test]
    fn conflict_refuses_to_plan() {
        let mut data = base_data();
        data.owner_conflict = Some(vec!["NetworkManager".to_string(), "ethtool".to_string()]);
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = NetworkWakeOnLan.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.conflict.is_some());
        let result = NetworkWakeOnLan.plan(&ctx, &observation(data), &diagnosis);
        assert!(result.is_err());
    }

    #[test]
    fn disabled_config_is_always_compliant() {
        let mut config = crate::config::DebkitConfig::default();
        config.wake_on_lan.enabled = false;
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let mut data = base_data();
        data.owner_conflict = Some(vec!["NetworkManager".to_string(), "ethtool".to_string()]);
        let diagnosis = NetworkWakeOnLan.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = NetworkWakeOnLan
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }
}
