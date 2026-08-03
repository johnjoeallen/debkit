use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::{Change, ChangePlan, Risk, ServiceActionKind};
use crate::install::nis as legacy;

/// Everything `diagnose()`/`plan()` need, captured once by `discover()`.
///
/// Map lifecycle (ypinit, ypservers/map rebuild, yppush, SSH slave registration) is
/// deliberately NOT modeled here yet — `master`/`slave` roles only get the file/service
/// layer (domain, yp.conf, nsswitch, package/service state) ported onto the declarative
/// engine. `maps_initialized` is surfaced as a diagnostic finding either way, but `plan()`
/// does not attempt to close it; the legacy `debkit install nis` / `debkit configure nis`
/// commands remain the way to manage maps until that's ported in a follow-up.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NisObservation {
    role: String,
    domain: String,
    master: Option<String>,
    defaultdomain_current: Option<String>,
    defaultdomain_desired: String,
    yp_conf_current: Option<String>,
    yp_conf_desired: String,
    nsswitch_current: Option<String>,
    nsswitch_desired: String,
    packages_missing: Vec<String>,
    services: Vec<ServiceState>,
    includes_server: bool,
    maps_initialized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceState {
    unit: String,
    enabled: bool,
    active: bool,
}

impl ServiceState {
    fn compliant(&self) -> bool {
        self.enabled && self.active
    }
}

pub struct IdentityNis;

impl Module for IdentityNis {
    fn name(&self) -> &'static str {
        "identity.nis"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let config = &ctx.config.nis;
        if !config.enabled {
            let mut observation = Observation::new(serde_json::json!({"enabled": false}));
            observation = observation.with_warning("NIS is disabled (`nis.enabled = false`)");
            return Ok(observation);
        }

        let plan = legacy::build_plan(legacy::Role::Configured, config)?;

        let defaultdomain_current = read_trimmed(legacy::DEFAULTDOMAIN_PATH);
        let defaultdomain_desired = format!("{}\n", plan.domain);

        let yp_conf_current = fs::read_to_string(legacy::YP_CONF_PATH).ok();
        let yp_conf_desired = legacy::render_yp_conf(&plan.domain, &plan.client_servers_as_strs());

        let nsswitch_raw = fs::read_to_string(legacy::NSSWITCH_PATH).unwrap_or_default();
        let nsswitch_desired = legacy::render_nsswitch_with_files_then_nis(&nsswitch_raw);
        let nsswitch_current = if nsswitch_raw.is_empty() {
            None
        } else {
            Some(nsswitch_raw)
        };

        let packages_missing: Vec<String> = plan
            .packages
            .iter()
            .filter(|package| !legacy::package_installed(package).unwrap_or(false))
            .map(|package| (*package).to_string())
            .collect();

        let mut services = Vec::new();
        for unit in &plan.services {
            services.push(ServiceState {
                unit: (*unit).to_string(),
                enabled: exec::systemctl_is_enabled(unit),
                active: exec::systemctl_is_active(unit),
            });
        }
        for unit in &plan.optional_services {
            if exec::systemd_unit_exists(&format!("{unit}.service")) {
                services.push(ServiceState {
                    unit: (*unit).to_string(),
                    enabled: exec::systemctl_is_enabled(unit),
                    active: exec::systemctl_is_active(unit),
                });
            }
        }

        let maps_initialized = std::path::Path::new(legacy::YP_MAP_ROOT)
            .join(&plan.domain)
            .exists();

        let data = NisObservation {
            role: plan.role.config_value().to_string(),
            domain: plan.domain.clone(),
            master: plan.master.clone(),
            defaultdomain_current,
            defaultdomain_desired,
            yp_conf_current,
            yp_conf_desired,
            nsswitch_current,
            nsswitch_desired,
            packages_missing,
            services,
            includes_server: plan.role.includes_server(),
            maps_initialized,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = Some("NIS".to_string());
        if data.includes_server && !data.maps_initialized {
            observation = observation.with_warning(format!(
                "NIS maps for `{}` are not yet initialized under {}; map lifecycle is managed by legacy `debkit install nis`, not this module",
                data.domain,
                legacy::YP_MAP_ROOT
            ));
        }
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.nis.enabled {
            return Diagnosis::compliant();
        }

        let data: NisObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read identity.nis observation: {err}"
                )]);
            }
        };

        let mut findings = Vec::new();
        if data.defaultdomain_current.as_deref() != Some(data.defaultdomain_desired.as_str()) {
            findings.push(format!(
                "{} does not declare domain `{}`",
                legacy::DEFAULTDOMAIN_PATH,
                data.domain
            ));
        }
        if data.yp_conf_current.as_deref() != Some(data.yp_conf_desired.as_str()) {
            findings.push(format!(
                "{} does not match the declared role",
                legacy::YP_CONF_PATH
            ));
        }
        if data.nsswitch_current.as_deref() != Some(data.nsswitch_desired.as_str()) {
            findings.push(format!(
                "{} does not keep local files before NIS for passwd/group/shadow",
                legacy::NSSWITCH_PATH
            ));
        }
        if !data.packages_missing.is_empty() {
            findings.push(format!(
                "missing packages: {}",
                data.packages_missing.join(", ")
            ));
        }
        for service in &data.services {
            if !service.compliant() {
                findings.push(format!(
                    "{} is not enabled and active (enabled={}, active={})",
                    service.unit, service.enabled, service.active
                ));
            }
        }
        if data.includes_server && !data.maps_initialized {
            findings.push(
                "NIS maps are not yet initialized (not managed by this module — run legacy `debkit install nis`)"
                    .to_string(),
            );
        }

        if findings.is_empty() {
            Diagnosis::compliant()
        } else {
            Diagnosis::mismatch(findings)
        }
    }

    fn plan(
        &self,
        ctx: &Context,
        observation: &Observation,
        diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        let mut plan = ChangePlan::new();
        if !ctx.config.nis.enabled || diagnosis.compliant {
            return Ok(plan);
        }

        let data: NisObservation = serde_json::from_value(observation.data.clone())?;

        if data.defaultdomain_current.as_deref() != Some(data.defaultdomain_desired.as_str()) {
            plan.push(
                format!("declare NIS domain `{}`", data.domain),
                Risk::Medium,
                Change::WriteFile {
                    path: PathBuf::from(legacy::DEFAULTDOMAIN_PATH),
                    content: data.defaultdomain_desired.clone(),
                },
            );
            plan.push(
                format!("set runtime NIS domain to `{}`", data.domain),
                Risk::Low,
                Change::RunCommand {
                    program: "domainname".to_string(),
                    args: vec![data.domain.clone()],
                    privileged: true,
                },
            );
        }

        if data.yp_conf_current.as_deref() != Some(data.yp_conf_desired.as_str()) {
            plan.push(
                format!("write {} for role `{}`", legacy::YP_CONF_PATH, data.role),
                Risk::Medium,
                Change::WriteFile {
                    path: PathBuf::from(legacy::YP_CONF_PATH),
                    content: data.yp_conf_desired.clone(),
                },
            );
        }

        if data.nsswitch_current.as_deref() != Some(data.nsswitch_desired.as_str()) {
            plan.push(
                format!(
                    "keep local files before NIS in {} for passwd/group/shadow",
                    legacy::NSSWITCH_PATH
                ),
                Risk::High,
                Change::WriteFile {
                    path: PathBuf::from(legacy::NSSWITCH_PATH),
                    content: data.nsswitch_desired.clone(),
                },
            );
        }

        if !data.packages_missing.is_empty() {
            plan.push(
                format!("install NIS packages: {}", data.packages_missing.join(", ")),
                Risk::Medium,
                Change::InstallPackages {
                    packages: data.packages_missing.clone(),
                },
            );
        }

        for service in &data.services {
            if !service.compliant() {
                plan.push(
                    format!("enable and start {}", service.unit),
                    Risk::Low,
                    Change::ServiceAction {
                        unit: service.unit.clone(),
                        action: ServiceActionKind::EnableNow,
                    },
                );
            }
        }

        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let config = &ctx.config.nis;
        if !config.enabled {
            return Ok(vec![VerificationResult::skipped(
                "nis",
                "disabled in config",
            )]);
        }

        let plan = legacy::build_plan(legacy::Role::Configured, config)?;
        let mut checks = Vec::new();

        match exec::capture("ypwhich", &[]) {
            Ok(server) => checks.push(VerificationResult::pass(format!(
                "ypwhich bound to `{}`",
                server.trim()
            ))),
            Err(err) => checks.push(VerificationResult::fail("ypwhich binding", err.to_string())),
        }

        if plan.role.includes_client() {
            let user = plan.admin_user.trim();
            for group in &plan.local_admin_groups {
                let group = group.trim();
                if group.is_empty() || user.is_empty() {
                    continue;
                }
                let check_name = format!("NIS group `{group}` membership for `{user}`");
                let group_output = exec::capture("getent", &["group", group]).unwrap_or_default();
                if !legacy::group_entry_lists_user(group, user, &group_output) {
                    checks.push(VerificationResult::skipped(
                        check_name,
                        format!("`{group}` does not list `{user}` as a member"),
                    ));
                    continue;
                }
                let initgroups_output =
                    exec::capture("getent", &["initgroups", user]).unwrap_or_default();
                let id_output = exec::capture("id", &[user]).unwrap_or_default();
                let initgroups_lists_group =
                    legacy::whitespace_fields_contain(&initgroups_output, group);
                let id_lists_group = legacy::id_output_contains_group(&id_output, group);
                if initgroups_lists_group && id_lists_group {
                    checks.push(VerificationResult::pass(check_name));
                } else {
                    checks.push(VerificationResult::fail(
                        check_name,
                        "supplementary group lookup did not include it",
                    ));
                }
            }
        }

        Ok(checks)
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|raw| {
        if raw.ends_with('\n') {
            raw
        } else {
            format!("{raw}\n")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(IdentityNis.name(), "identity.nis");
    }

    fn observation(data: NisObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn base_data() -> NisObservation {
        NisObservation {
            role: "slave".to_string(),
            domain: "dublinux.lan".to_string(),
            master: Some("iris.dublinux.lan".to_string()),
            defaultdomain_current: Some("dublinux.lan\n".to_string()),
            defaultdomain_desired: "dublinux.lan\n".to_string(),
            yp_conf_current: Some("domain dublinux.lan server 127.0.0.1\n".to_string()),
            yp_conf_desired: "domain dublinux.lan server 127.0.0.1\n".to_string(),
            nsswitch_current: Some("passwd:         files nis\n".to_string()),
            nsswitch_desired: "passwd:         files nis\n".to_string(),
            packages_missing: Vec::new(),
            services: vec![ServiceState {
                unit: "ypbind".to_string(),
                enabled: true,
                active: true,
            }],
            includes_server: true,
            maps_initialized: true,
        }
    }

    fn config() -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.nis.enabled = true;
        config.nis.role = "slave".to_string();
        config.nis.domain = "dublinux.lan".to_string();
        config.nis.master = "iris.dublinux.lan".to_string();
        config
    }

    #[test]
    fn compliant_when_everything_matches() {
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentityNis.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
        let plan = IdentityNis
            .plan(&ctx, &observation(base_data()), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn drifted_yp_conf_produces_a_write_change() {
        let mut data = base_data();
        data.yp_conf_current = Some("domain other.lan server 1.2.3.4\n".to_string());
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentityNis.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = IdentityNis
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 1);
        match &plan.changes[0].change {
            Change::WriteFile { path, .. } => {
                assert_eq!(path, &PathBuf::from(legacy::YP_CONF_PATH));
            }
            other => panic!("expected WriteFile, got {other:?}"),
        }
    }

    #[test]
    fn missing_packages_and_inactive_service_produce_expected_changes() {
        let mut data = base_data();
        data.packages_missing = vec!["ypbind-mt".to_string()];
        data.services = vec![ServiceState {
            unit: "ypbind".to_string(),
            enabled: false,
            active: false,
        }];
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentityNis.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = IdentityNis
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 2);
        assert!(matches!(
            plan.changes[0].change,
            Change::InstallPackages { .. }
        ));
        assert!(matches!(
            plan.changes[1].change,
            Change::ServiceAction { .. }
        ));
    }

    #[test]
    fn uninitialized_maps_are_flagged_but_not_planned() {
        let mut data = base_data();
        data.maps_initialized = false;
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentityNis.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|finding| finding.contains("not yet initialized"))
        );
        let plan = IdentityNis
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty(), "map init is not ported to plan() yet");
    }

    #[test]
    fn disabled_config_is_always_compliant() {
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let mut data = base_data();
        data.yp_conf_current = Some("garbage".to_string());
        let diagnosis = IdentityNis.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = IdentityNis
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }
}
