use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::ChangePlan;

/// Well-documented, stable signal that `tailscaled` took over `/etc/resolv.conf`
/// (it writes the pre-existing file here before replacing it).
const RESOLV_CONF_BACKUP_PATH: &str = "/etc/resolv.pre-tailscale-backup.conf";

/// Deliberately read-only beyond the "installed and running" check. `tailscale status
/// --json` is a stable, documented interface and is used directly; the specific CLI
/// surface for changing DNS acceptance behavior (`tailscale set --accept-dns=...`) is
/// real but its interaction with `magic_dns_off_lan`/`preserve_lan_dns` (an on-LAN vs.
/// off-LAN split Tailscale itself doesn't natively express) isn't something this module
/// can confidently automate without a live Tailscale install to validate against — this
/// dev host doesn't have one. `plan()`/`apply()` stay empty; the config fields are
/// surfaced as diagnostic context, not enforced.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TailscaleObservation {
    installed: bool,
    version: Option<String>,
    backend_state: Option<String>,
    self_dns_name: Option<String>,
    self_tailscale_ips: Vec<String>,
    magic_dns_suffix: Option<String>,
    resolv_conf_owned_by_tailscale: bool,
    resolv_conf_current: Option<String>,
}

pub struct NetworkTailscale;

impl Module for NetworkTailscale {
    fn name(&self) -> &'static str {
        "network.tailscale"
    }

    fn description(&self) -> &'static str {
        "read-only Tailscale backend/DNS status via `tailscale status --json`"
    }

    fn discover(&self, _ctx: &Context) -> anyhow::Result<Observation> {
        let installed = exec::command_available("tailscale");
        let version = installed
            .then(|| exec::capture("tailscale", &["version"]).ok())
            .flatten()
            .map(|raw| raw.lines().next().unwrap_or_default().trim().to_string());

        let status = installed
            .then(|| exec::capture("tailscale", &["status", "--json"]).ok())
            .flatten()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());

        let backend_state = status
            .as_ref()
            .and_then(|value| value.get("BackendState"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let self_dns_name = status
            .as_ref()
            .and_then(|value| value.get("Self"))
            .and_then(|self_value| self_value.get("DNSName"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let self_tailscale_ips = status
            .as_ref()
            .and_then(|value| value.get("Self"))
            .and_then(|self_value| self_value.get("TailscaleIPs"))
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let magic_dns_suffix = status
            .as_ref()
            .and_then(|value| value.get("MagicDNSSuffix"))
            .and_then(|value| value.as_str())
            .map(str::to_string);

        let resolv_conf_owned_by_tailscale = Path::new(RESOLV_CONF_BACKUP_PATH).exists();
        let resolv_conf_current = std::fs::read_to_string("/etc/resolv.conf").ok();

        let data = TailscaleObservation {
            installed,
            version,
            backend_state,
            self_dns_name,
            self_tailscale_ips,
            magic_dns_suffix,
            resolv_conf_owned_by_tailscale,
            resolv_conf_current,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = if data.resolv_conf_owned_by_tailscale {
            Some("tailscaled".to_string())
        } else {
            None
        };
        if installed && data.backend_state.is_none() {
            observation = observation.with_warning(
                "tailscale is installed but `tailscale status --json` did not report a backend state"
                    .to_string(),
            );
        }
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.tailscale.enabled {
            return Diagnosis::compliant();
        }

        let data: TailscaleObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read network.tailscale observation: {err}"
                )]);
            }
        };

        let mut findings = Vec::new();
        if !data.installed {
            findings.push("tailscale is not installed".to_string());
        } else if data.backend_state.as_deref() != Some("Running") {
            findings.push(format!(
                "tailscale backend is not Running (state: {})",
                data.backend_state.as_deref().unwrap_or("unknown")
            ));
        }

        if findings.is_empty() {
            Diagnosis::compliant()
        } else {
            Diagnosis::mismatch(findings)
        }
    }

    fn plan(
        &self,
        _ctx: &Context,
        _observation: &Observation,
        _diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        // Installing/starting tailscaled is left to the operator; see module doc comment
        // for why DNS-acceptance behavior isn't automated here either.
        Ok(ChangePlan::new())
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        if !ctx.config.tailscale.enabled {
            return Ok(vec![VerificationResult::skipped(
                "network.tailscale",
                "disabled in config",
            )]);
        }

        let mut checks = Vec::new();
        if !exec::command_available("tailscale") {
            checks.push(VerificationResult::fail(
                "tailscale is installed",
                "`tailscale` was not found on PATH",
            ));
            return Ok(checks);
        }
        checks.push(VerificationResult::pass("tailscale is installed"));

        match exec::capture("tailscale", &["status", "--json"])
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|value| {
                value
                    .get("BackendState")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            }) {
            Some(state) if state == "Running" => {
                checks.push(VerificationResult::pass("tailscale backend is Running"));
            }
            Some(state) => checks.push(VerificationResult::fail(
                "tailscale backend is Running",
                format!("backend state is `{state}`"),
            )),
            None => checks.push(VerificationResult::fail(
                "tailscale backend is Running",
                "could not read backend state from `tailscale status --json`",
            )),
        }

        Ok(checks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(NetworkTailscale.name(), "network.tailscale");
    }

    fn observation(data: TailscaleObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn base_data() -> TailscaleObservation {
        TailscaleObservation {
            installed: true,
            version: Some("1.70.0".to_string()),
            backend_state: Some("Running".to_string()),
            self_dns_name: Some("tornado.tailnet.ts.net.".to_string()),
            self_tailscale_ips: vec!["100.64.0.1".to_string()],
            magic_dns_suffix: Some("tailnet.ts.net".to_string()),
            resolv_conf_owned_by_tailscale: true,
            resolv_conf_current: Some("nameserver 100.100.100.100\n".to_string()),
        }
    }

    fn config() -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.tailscale.enabled = true;
        config
    }

    #[test]
    fn compliant_when_installed_and_running() {
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = NetworkTailscale.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
    }

    #[test]
    fn mismatch_when_not_installed() {
        let mut data = base_data();
        data.installed = false;
        data.backend_state = None;
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = NetworkTailscale.diagnose(&ctx, &observation(data));
        assert!(!diagnosis.compliant);
        assert!(diagnosis.findings[0].contains("not installed"));
    }

    #[test]
    fn mismatch_when_backend_not_running() {
        let mut data = base_data();
        data.backend_state = Some("NeedsLogin".to_string());
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = NetworkTailscale.diagnose(&ctx, &observation(data));
        assert!(!diagnosis.compliant);
        assert!(diagnosis.findings[0].contains("NeedsLogin"));
    }

    #[test]
    fn plan_is_always_empty() {
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = NetworkTailscale.diagnose(&ctx, &observation(base_data()));
        let plan = NetworkTailscale
            .plan(&ctx, &observation(base_data()), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn disabled_config_is_always_compliant() {
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let mut data = base_data();
        data.installed = false;
        let diagnosis = NetworkTailscale.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }
}
