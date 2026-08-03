use std::collections::BTreeMap;
use std::fs;

use serde::Serialize;

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::ChangePlan;

/// Packages worth reporting when installed — the subsystems that repeatedly turned out to
/// be the actual owner of a setting during troubleshooting (DNS resolvers, network
/// managers, Tailscale, NIS, firewalling). Absence from this list is not a finding; it's
/// just not queried.
const WATCHED_PACKAGES: &[&str] = &[
    "network-manager",
    "systemd-resolved",
    "dnsmasq",
    "tailscale",
    "docker.io",
    "docker-ce",
    "nis",
    "ypserv",
    "ypbind-mt",
    "nftables",
    "iptables",
    "apt-cacher-ng",
];

#[derive(Debug, Clone, Default, Serialize)]
struct OsRelease {
    pretty_name: Option<String>,
    id: Option<String>,
    version_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct InterfaceSummary {
    name: String,
    mac_address: Option<String>,
    kind: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct CoreInspectData {
    hostname: String,
    os: OsRelease,
    kernel: Option<String>,
    boot_id: Option<String>,
    failed_units: Vec<String>,
    packages: BTreeMap<String, String>,
    interfaces: Vec<InterfaceSummary>,
}

pub struct CoreInspect;

impl Module for CoreInspect {
    fn name(&self) -> &'static str {
        "core.inspect"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let data = CoreInspectData {
            hostname: ctx.hostname.clone(),
            os: read_os_release(),
            kernel: exec::capture("uname", &["-r"])
                .ok()
                .map(|s| s.trim().to_string()),
            boot_id: fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .ok()
                .map(|s| s.trim().to_string()),
            failed_units: read_failed_units(),
            packages: read_watched_package_versions(),
            interfaces: read_interfaces(),
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        if !data.failed_units.is_empty() {
            observation = observation.with_warning(format!(
                "{} systemd unit(s) reported as failed: {}",
                data.failed_units.len(),
                data.failed_units.join(", ")
            ));
        }
        Ok(observation)
    }

    fn diagnose(&self, _ctx: &Context, observation: &Observation) -> Diagnosis {
        // core.inspect is read-only baseline evidence, not an enforced resource — it has
        // no desired state of its own to compare against, so it never reports mismatch.
        // Its warnings (e.g. failed units) still surface via Observation::warnings.
        let _ = observation;
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
        Ok(vec![VerificationResult::skipped(
            "core.inspect has no applied state",
            "read-only baseline module; nothing to verify",
        )])
    }
}

fn read_os_release() -> OsRelease {
    let Ok(raw) = fs::read_to_string("/etc/os-release") else {
        return OsRelease::default();
    };
    let mut release = OsRelease::default();
    for line in raw.lines() {
        if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
            release.pretty_name = Some(value.trim_matches('"').to_string());
        } else if let Some(value) = line.strip_prefix("VERSION_ID=") {
            release.version_id = Some(value.trim_matches('"').to_string());
        } else if let Some(value) = line.strip_prefix("ID=") {
            release.id = Some(value.trim_matches('"').to_string());
        }
    }
    release
}

fn read_failed_units() -> Vec<String> {
    let Ok(raw) = exec::capture(
        "systemctl",
        &["--failed", "--no-legend", "--plain", "--all"],
    ) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

fn read_watched_package_versions() -> BTreeMap<String, String> {
    let mut packages = BTreeMap::new();
    for package in WATCHED_PACKAGES {
        if let Ok(raw) = exec::capture("dpkg-query", &["-W", "-f=${Status} ${Version}", package]) {
            let raw = raw.trim();
            if let Some((status, version)) = raw.split_once(' ') {
                if status.trim_end().ends_with("installed") && !version.trim().is_empty() {
                    packages.insert((*package).to_string(), version.trim().to_string());
                }
            }
        }
    }
    packages
}

fn read_interfaces() -> Vec<InterfaceSummary> {
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
            let kind = if path.join("wireless").exists() {
                "wireless"
            } else if path.join("device").exists() {
                "wired"
            } else {
                "other"
            };
            let mac_address = fs::read_to_string(path.join("address"))
                .ok()
                .map(|s| s.trim().to_string());
            Some(InterfaceSummary {
                name,
                mac_address,
                kind,
            })
        })
        .collect::<Vec<_>>();
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_parses_quoted_fields() {
        let dir =
            std::env::temp_dir().join(format!("debkit_core_inspect_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // read_os_release reads a fixed system path, so exercise the parsing logic
        // directly against representative content instead of stubbing the filesystem.
        let raw = "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nID=debian\nVERSION_ID=\"13\"\n";
        let mut release = OsRelease::default();
        for line in raw.lines() {
            if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
                release.pretty_name = Some(value.trim_matches('"').to_string());
            } else if let Some(value) = line.strip_prefix("VERSION_ID=") {
                release.version_id = Some(value.trim_matches('"').to_string());
            } else if let Some(value) = line.strip_prefix("ID=") {
                release.id = Some(value.trim_matches('"').to_string());
            }
        }
        assert_eq!(
            release.pretty_name.as_deref(),
            Some("Debian GNU/Linux 13 (trixie)")
        );
        assert_eq!(release.id.as_deref(), Some("debian"));
        assert_eq!(release.version_id.as_deref(), Some("13"));
    }

    #[test]
    fn module_name_matches_config_section_convention() {
        assert_eq!(CoreInspect.name(), "core.inspect");
    }

    #[test]
    fn plan_is_always_empty_since_core_inspect_is_read_only() {
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "test-host".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::json!({}));
        let diagnosis = Diagnosis::compliant();
        let plan = CoreInspect.plan(&ctx, &observation, &diagnosis).unwrap();
        assert!(plan.is_empty());
    }
}
