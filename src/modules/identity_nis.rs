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
/// Map lifecycle is now ported for the **master** role: `ypinit -m` (when
/// uninitialized), the `ypservers` source file and its rebuilt map, `make -C /var/yp`,
/// and (when configured) `yppush` to declared slaves.
///
/// **Slave-side map lifecycle stays deliberately deferred** — the bootstrap-then-finalize
/// `yp.conf` dance, `ypinit -s` with its `ypxfr` fallback, and SSH-based master
/// registration are stateful, multi-host, and touch a live remote host (the master) on
/// top of the slave itself. That's a materially different risk profile from the
/// self-contained master-side steps, so it remains the domain of legacy `debkit install
/// nis` / `debkit configure nis`. What *did* need fixing regardless: `yp_conf_desired`
/// now accounts for `maps_initialized` so a slave with no local maps yet is never told to
/// prefer `127.0.0.1` before local `ypserv` actually has something to answer with — the
/// exact failure mode the requirements doc calls out ("do not prefer localhost on a slave
/// before local maps exist").
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
    /// Master role only: this host's own FQDN, used to render the `ypservers` map.
    master_fqdn: Option<String>,
    ypservers_source_current: Option<String>,
    ypservers_source_desired: Option<String>,
    ypservers_makedbm_input: Option<String>,
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

    fn config_key(&self) -> Option<&'static str> {
        Some("nis")
    }

    fn description(&self) -> &'static str {
        "NIS domain, yp.conf, nsswitch.conf, and master-side map lifecycle"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let config = &ctx.config.nis;
        if !config.enabled {
            let mut observation = Observation::new(serde_json::json!({"enabled": false}));
            observation = observation.with_warning("NIS is disabled (`nis.enabled = false`)");
            return Ok(observation);
        }

        let plan = legacy::build_plan(legacy::Role::Configured, config)?;

        let maps_initialized = std::path::Path::new(legacy::YP_MAP_ROOT)
            .join(&plan.domain)
            .exists();

        let defaultdomain_current = read_trimmed(legacy::DEFAULTDOMAIN_PATH);
        let defaultdomain_desired = format!("{}\n", plan.domain);

        let yp_conf_current = fs::read_to_string(legacy::YP_CONF_PATH).ok();
        let yp_conf_desired = desired_yp_conf(
            plan.role,
            &plan.domain,
            plan.master.as_deref(),
            &plan.client_servers_as_strs(),
            maps_initialized,
        );

        let (
            master_fqdn,
            ypservers_source_current,
            ypservers_source_desired,
            ypservers_makedbm_input,
        ) = if matches!(plan.role, legacy::NisRole::Master) {
            let fqdn = legacy::current_fqdn(&plan.domain).ok();
            let current = fs::read_to_string(legacy::YPSERVERS_SOURCE_PATH).ok();
            let desired = fqdn
                .as_deref()
                .map(|master| legacy::render_ypservers_source(master, &plan.slaves));
            let makedbm_input = fqdn
                .as_deref()
                .map(|master| legacy::render_ypservers_makedbm_input(master, &plan.slaves));
            (fqdn, current, desired, makedbm_input)
        } else {
            (None, None, None, None)
        };

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
            master_fqdn,
            ypservers_source_current,
            ypservers_source_desired,
            ypservers_makedbm_input,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = Some("NIS".to_string());
        if matches!(plan.role, legacy::NisRole::Master) && !data.maps_initialized {
            observation = observation.with_warning(format!(
                "NIS master maps for `{}` are not yet initialized under {}",
                data.domain,
                legacy::YP_MAP_ROOT
            ));
        } else if data.includes_server && !data.maps_initialized {
            observation = observation.with_warning(format!(
                "NIS maps for `{}` are not yet initialized under {}; slave map sync is managed by legacy `debkit install nis`, not this module",
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
        if data.role == "master" {
            if !data.maps_initialized {
                findings.push("NIS master maps are not yet initialized".to_string());
            }
            if data.ypservers_source_current != data.ypservers_source_desired {
                findings.push(format!(
                    "{} does not match the expected ypservers list",
                    legacy::YPSERVERS_SOURCE_PATH
                ));
            }
        } else if data.includes_server && !data.maps_initialized {
            findings.push(
                "NIS maps are not yet initialized (slave map sync is not managed by this module — run legacy `debkit install nis`)"
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

        if data.role == "master" {
            push_master_map_lifecycle(&mut plan, &ctx.config.nis, &data);
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

/// A slave with no local maps yet must not prefer `127.0.0.1` — local `ypserv` has
/// nothing to answer with. Bootstraps against `master` alone until maps exist; every
/// other case (master, client, or a slave whose maps are already initialized) gets the
/// normal final `client_servers` form.
fn desired_yp_conf(
    role: legacy::NisRole,
    domain: &str,
    master: Option<&str>,
    client_servers: &[&str],
    maps_initialized: bool,
) -> String {
    if matches!(role, legacy::NisRole::Slave) && !maps_initialized {
        legacy::render_yp_conf(domain, &[master.unwrap_or_default()])
    } else {
        legacy::render_yp_conf(domain, client_servers)
    }
}

/// Master-role map lifecycle: `ypinit -m` (only if uninitialized), the `ypservers`
/// source file and its rebuilt map, `make -C /var/yp`, and (when configured) `yppush` to
/// declared slaves. The rebuild/push steps run whenever maps are freshly initializing or
/// the `ypservers` list itself changed — matching legacy's own choice to delegate real
/// rebuild idempotency to `make`'s dependency graph rather than reinventing it here.
fn push_master_map_lifecycle(
    plan: &mut ChangePlan,
    nis_config: &crate::config::NisConfig,
    data: &NisObservation,
) {
    let rebuild_needed =
        !data.maps_initialized || data.ypservers_source_current != data.ypservers_source_desired;

    if !data.maps_initialized {
        plan.push(
            "initialize NIS master maps (ypinit -m)",
            Risk::High,
            Change::RunCommand {
                program: legacy::YPINIT_PATH.to_string(),
                args: vec!["-m".to_string()],
                privileged: true,
            },
        );
    }

    if let Some(desired) = &data.ypservers_source_desired
        && data.ypservers_source_current.as_deref() != Some(desired.as_str())
    {
        plan.push(
            format!("write {}", legacy::YPSERVERS_SOURCE_PATH),
            Risk::Medium,
            Change::WriteFile {
                path: PathBuf::from(legacy::YPSERVERS_SOURCE_PATH),
                content: desired.clone(),
            },
        );
    }

    if !rebuild_needed {
        return;
    }

    if let Some(makedbm_input) = &data.ypservers_makedbm_input {
        let tmp_path = format!("/tmp/debkit-ypservers-{}.map", std::process::id());
        let target = format!("{}/{}/ypservers", legacy::YP_MAP_ROOT, data.domain);
        plan.push(
            "stage ypservers makedbm input",
            Risk::Low,
            Change::WriteFile {
                path: PathBuf::from(&tmp_path),
                content: makedbm_input.clone(),
            },
        );
        plan.push(
            "rebuild ypservers map",
            Risk::Medium,
            Change::RunCommand {
                program: legacy::MAKEDBM_PATH.to_string(),
                args: vec![tmp_path.clone(), target],
                privileged: true,
            },
        );
        plan.push(
            "remove temporary ypservers input file",
            Risk::Low,
            Change::RunCommand {
                program: "rm".to_string(),
                args: vec!["-f".to_string(), tmp_path],
                privileged: true,
            },
        );
    }

    plan.push(
        "rebuild NIS master maps",
        Risk::Medium,
        Change::RunCommand {
            program: "make".to_string(),
            args: vec!["-C".to_string(), legacy::YP_MAP_ROOT.to_string()],
            privileged: true,
        },
    );

    if nis_config.push_to_slaves && !nis_config.slaves.is_empty() {
        for slave in &nis_config.slaves {
            for map in legacy::FALLBACK_MAPS {
                plan.push(
                    format!("push map {map} to {slave}"),
                    Risk::Low,
                    Change::RunCommand {
                        program: legacy::YPPUSH_PATH.to_string(),
                        args: vec![
                            "-d".to_string(),
                            data.domain.clone(),
                            "-h".to_string(),
                            slave.clone(),
                            (*map).to_string(),
                        ],
                        privileged: true,
                    },
                );
            }
        }
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
            master_fqdn: None,
            ypservers_source_current: None,
            ypservers_source_desired: None,
            ypservers_makedbm_input: None,
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

    #[test]
    fn slave_with_uninitialized_maps_bootstraps_against_master_only() {
        let rendered = desired_yp_conf(
            legacy::NisRole::Slave,
            "dublinux.lan",
            Some("iris.dublinux.lan"),
            &["127.0.0.1", "iris.dublinux.lan"],
            false,
        );
        assert_eq!(
            rendered,
            legacy::render_yp_conf("dublinux.lan", &["iris.dublinux.lan"])
        );
        assert!(!rendered.contains("127.0.0.1"));
    }

    #[test]
    fn slave_with_initialized_maps_uses_final_client_servers() {
        let rendered = desired_yp_conf(
            legacy::NisRole::Slave,
            "dublinux.lan",
            Some("iris.dublinux.lan"),
            &["127.0.0.1", "iris.dublinux.lan"],
            true,
        );
        assert!(rendered.contains("127.0.0.1"));
        assert!(rendered.contains("iris.dublinux.lan"));
    }

    #[test]
    fn master_role_ignores_maps_initialized_for_yp_conf() {
        let rendered = desired_yp_conf(
            legacy::NisRole::Master,
            "dublinux.lan",
            None,
            &["127.0.0.1"],
            false,
        );
        assert_eq!(
            rendered,
            legacy::render_yp_conf("dublinux.lan", &["127.0.0.1"])
        );
    }

    fn master_data() -> NisObservation {
        let mut data = base_data();
        data.role = "master".to_string();
        data.master = None;
        data.yp_conf_current = Some("domain dublinux.lan server 127.0.0.1\n".to_string());
        data.yp_conf_desired = "domain dublinux.lan server 127.0.0.1\n".to_string();
        data.master_fqdn = Some("iris.dublinux.lan".to_string());
        let ypservers_source = legacy::render_ypservers_source("iris.dublinux.lan", &[]);
        data.ypservers_source_current = Some(ypservers_source.clone());
        data.ypservers_source_desired = Some(ypservers_source);
        data.ypservers_makedbm_input = Some(legacy::render_ypservers_makedbm_input(
            "iris.dublinux.lan",
            &[],
        ));
        data.maps_initialized = true;
        data
    }

    fn master_config() -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.nis.enabled = true;
        config.nis.role = "master".to_string();
        config.nis.domain = "dublinux.lan".to_string();
        config
    }

    #[test]
    fn compliant_master_with_matching_ypservers_plans_nothing() {
        let config = master_config();
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let diagnosis = IdentityNis.diagnose(&ctx, &observation(master_data()));
        assert!(diagnosis.compliant);
        let plan = IdentityNis
            .plan(&ctx, &observation(master_data()), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn uninitialized_master_maps_plan_ypinit_then_rebuild() {
        let mut data = master_data();
        data.maps_initialized = false;
        let config = master_config();
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let diagnosis = IdentityNis.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|finding| finding.contains("master maps are not yet initialized"))
        );
        let plan = IdentityNis
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();

        let programs: Vec<&str> = plan
            .changes
            .iter()
            .filter_map(|planned| match &planned.change {
                Change::RunCommand { program, .. } => Some(program.as_str()),
                _ => None,
            })
            .collect();
        assert!(programs.contains(&legacy::YPINIT_PATH));
        assert!(programs.contains(&legacy::MAKEDBM_PATH));
        assert!(programs.contains(&"make"));
        // ypinit must run before the rebuild steps.
        let ypinit_pos = programs
            .iter()
            .position(|p| *p == legacy::YPINIT_PATH)
            .unwrap();
        let make_pos = programs.iter().position(|p| *p == "make").unwrap();
        assert!(ypinit_pos < make_pos);
    }

    #[test]
    fn ypservers_drift_triggers_rebuild_without_ypinit() {
        let mut data = master_data();
        data.ypservers_source_current = Some("stale-host\tstale-host\n".to_string());
        let config = master_config();
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let diagnosis = IdentityNis.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = IdentityNis
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();

        let programs: Vec<&str> = plan
            .changes
            .iter()
            .filter_map(|planned| match &planned.change {
                Change::RunCommand { program, .. } => Some(program.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !programs.contains(&legacy::YPINIT_PATH),
            "maps already initialized"
        );
        assert!(programs.contains(&legacy::MAKEDBM_PATH));
        assert!(programs.contains(&"make"));
        assert!(plan.changes.iter().any(|planned| matches!(
            &planned.change,
            Change::WriteFile { path, .. } if path == &PathBuf::from(legacy::YPSERVERS_SOURCE_PATH)
        )));
    }

    #[test]
    fn push_to_slaves_adds_yppush_per_map_per_slave() {
        let mut data = master_data();
        data.maps_initialized = false;
        let mut config = master_config();
        config.nis.push_to_slaves = true;
        config.nis.slaves = vec!["spitfire.dublinux.lan".to_string()];
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let diagnosis = IdentityNis.diagnose(&ctx, &observation(data.clone()));
        let plan = IdentityNis
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();

        let push_count = plan
            .changes
            .iter()
            .filter(|planned| {
                matches!(&planned.change, Change::RunCommand { program, .. } if program == legacy::YPPUSH_PATH)
            })
            .count();
        assert_eq!(push_count, legacy::FALLBACK_MAPS.len());
    }
}
