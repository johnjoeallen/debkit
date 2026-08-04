use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::{Change, ChangePlan, Risk};
use crate::install::sudo_nopass as legacy;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SudoObservation {
    group: String,
    nis_managed: bool,
    group_exists_locally: bool,
    standard_sudo_rule_present: bool,
    legacy_file_present: bool,
    nopass_rule_current: Option<String>,
    nopass_rule_desired: String,
    nopass_rule_path: String,
    effective_users: Vec<String>,
    users_missing: Vec<String>,
    users_not_in_group: Vec<String>,
}

pub struct IdentitySudo;

impl Module for IdentitySudo {
    fn name(&self) -> &'static str {
        "identity.sudo"
    }

    fn description(&self) -> &'static str {
        "passwordless-sudo group, NOPASSWD drop-in, and membership management"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let config = &ctx.config.sudo_nopass;
        if !config.enabled {
            let mut observation = Observation::new(serde_json::json!({"enabled": false}));
            observation = observation
                .with_warning("passwordless sudo is disabled (`sudo_nopass.enabled = false`)");
            return Ok(observation);
        }
        if config.group.trim().is_empty() {
            anyhow::bail!("`sudo_nopass.group` must not be empty");
        }

        let group = config.group.trim().to_string();
        let group_exists_locally = legacy::local_group_exists(&group);
        let standard_sudo_rule_present = std::fs::read_to_string(legacy::SUDOERS_MAIN_PATH)
            .ok()
            .map(|content| {
                content.lines().any(|line| {
                    let line = line.trim();
                    line == "%sudo ALL=(ALL:ALL) ALL"
                        || line == "%sudo ALL=(ALL) ALL"
                        || line == "%sudo\tALL=(ALL:ALL) ALL"
                })
            })
            .unwrap_or(false);
        let legacy_file_present = std::path::Path::new(legacy::LEGACY_NOPASS_PATH).exists();

        let nopass_rule_path = format!("{}/99-{group}-nopass", legacy::SUDOERS_DROPIN_DIR);
        let nopass_rule_current = std::fs::read_to_string(&nopass_rule_path).ok();
        let nopass_rule_desired = legacy::render_group_nopass_rule(&group);

        let effective_users = legacy::effective_users(config);
        let mut users_missing = Vec::new();
        let mut users_not_in_group = Vec::new();
        if !config.nis_managed {
            for user in &effective_users {
                if !legacy::user_exists(user) {
                    users_missing.push(user.clone());
                    continue;
                }
                if !legacy::user_is_in_group(user, &group).unwrap_or(false) {
                    users_not_in_group.push(user.clone());
                }
            }
        }

        let data = SudoObservation {
            group,
            nis_managed: config.nis_managed,
            group_exists_locally,
            standard_sudo_rule_present,
            legacy_file_present,
            nopass_rule_current,
            nopass_rule_desired,
            nopass_rule_path,
            effective_users,
            users_missing,
            users_not_in_group,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = Some("sudoers.d".to_string());
        if !data.users_missing.is_empty() {
            observation = observation.with_warning(format!(
                "configured user(s) not found on this host, skipped: {}",
                data.users_missing.join(", ")
            ));
        }
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.sudo_nopass.enabled {
            return Diagnosis::compliant();
        }

        let data: SudoObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read identity.sudo observation: {err}"
                )]);
            }
        };

        let mut findings = Vec::new();
        if !data.nis_managed && !data.group_exists_locally {
            findings.push(format!("group `{}` does not exist locally", data.group));
        }
        if !data.standard_sudo_rule_present {
            findings.push(format!(
                "{} is missing the standard `%sudo` rule",
                legacy::SUDOERS_MAIN_PATH
            ));
        }
        if data.legacy_file_present {
            findings.push(format!(
                "legacy {} should be removed",
                legacy::LEGACY_NOPASS_PATH
            ));
        }
        if data.nopass_rule_current.as_deref() != Some(data.nopass_rule_desired.as_str()) {
            findings.push(format!(
                "{} does not match the declared NOPASSWD rule",
                data.nopass_rule_path
            ));
        }
        if !data.nis_managed && !data.users_not_in_group.is_empty() {
            findings.push(format!(
                "user(s) not yet in group `{}`: {}",
                data.group,
                data.users_not_in_group.join(", ")
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
        ctx: &Context,
        observation: &Observation,
        diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        let mut plan = ChangePlan::new();
        if !ctx.config.sudo_nopass.enabled || diagnosis.compliant {
            return Ok(plan);
        }

        let data: SudoObservation = serde_json::from_value(observation.data.clone())?;

        if !data.nis_managed && !data.group_exists_locally {
            plan.push(
                format!("create group `{}`", data.group),
                Risk::Medium,
                Change::RunCommand {
                    program: "groupadd".to_string(),
                    args: vec![data.group.clone()],
                    privileged: true,
                },
            );
        }

        if !data.standard_sudo_rule_present {
            push_secured_dropin(
                &mut plan,
                "install the standard %sudo rule",
                legacy::SUDOERS_STD_GROUP_PATH,
                "%sudo ALL=(ALL:ALL) ALL\n".to_string(),
            );
        }

        if data.legacy_file_present {
            plan.push(
                format!("remove legacy {}", legacy::LEGACY_NOPASS_PATH),
                Risk::Low,
                Change::RunCommand {
                    program: "rm".to_string(),
                    args: vec![legacy::LEGACY_NOPASS_PATH.to_string()],
                    privileged: true,
                },
            );
        }

        if data.nopass_rule_current.as_deref() != Some(data.nopass_rule_desired.as_str()) {
            push_secured_dropin(
                &mut plan,
                format!("write NOPASSWD rule for group `{}`", data.group),
                &data.nopass_rule_path,
                data.nopass_rule_desired.clone(),
            );
        }

        if !data.nis_managed {
            for user in &data.users_not_in_group {
                plan.push(
                    format!("add `{user}` to group `{}`", data.group),
                    Risk::Medium,
                    Change::RunCommand {
                        program: "usermod".to_string(),
                        args: vec!["-aG".to_string(), data.group.clone(), user.clone()],
                        privileged: true,
                    },
                );
            }
        }

        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let config = &ctx.config.sudo_nopass;
        if !config.enabled {
            return Ok(vec![VerificationResult::skipped(
                "identity.sudo",
                "disabled in config",
            )]);
        }

        let mut checks = Vec::new();
        match exec::capture("visudo", &["-c"]) {
            Ok(_) => checks.push(VerificationResult::pass("visudo -c")),
            Err(err) => checks.push(VerificationResult::fail("visudo -c", err.to_string())),
        }

        let group = config.group.trim();
        if !group.is_empty() {
            match exec::capture("getent", &["group", group]) {
                Ok(raw)
                    if raw
                        .lines()
                        .any(|line| line.starts_with(&format!("{group}:"))) =>
                {
                    checks.push(VerificationResult::pass(format!("getent group {group}")));
                }
                Ok(_) => checks.push(VerificationResult::fail(
                    format!("getent group {group}"),
                    "succeeded but did not return the expected group",
                )),
                Err(err) => checks.push(VerificationResult::fail(
                    format!("getent group {group}"),
                    err.to_string(),
                )),
            }

            for user in legacy::effective_users(config) {
                let check_name = format!("`{user}` reports membership in `{group}`");
                match legacy::user_is_in_group(&user, group) {
                    Ok(true) => checks.push(VerificationResult::pass(check_name)),
                    Ok(false) => checks.push(VerificationResult::fail(
                        check_name,
                        "id -nG did not report this group; sudo will ask for a password until NSS reports it",
                    )),
                    Err(err) => checks.push(VerificationResult::fail(check_name, err.to_string())),
                }

                let policy_check = format!("sudo NOPASSWD policy applies to `{user}`");
                if exec::current_euid().ok() == Some(0) {
                    match legacy::sudo_policy_allows_nopass(&user) {
                        Ok(true) => checks.push(VerificationResult::pass(policy_check)),
                        Ok(false) => checks.push(VerificationResult::fail(
                            policy_check,
                            "`sudo -n -l -U` did not show a NOPASSWD rule",
                        )),
                        Err(err) => {
                            checks.push(VerificationResult::fail(policy_check, err.to_string()))
                        }
                    }
                } else {
                    checks.push(VerificationResult::skipped(
                        policy_check,
                        "DebKit is not running as root",
                    ));
                }
            }
        }

        Ok(checks)
    }
}

fn push_secured_dropin(
    plan: &mut ChangePlan,
    description: impl Into<String>,
    path: &str,
    content: String,
) {
    plan.push(
        description,
        Risk::High,
        Change::WriteFile {
            path: PathBuf::from(path),
            content,
        },
    );
    plan.push(
        format!("secure {path} ownership"),
        Risk::Low,
        Change::RunCommand {
            program: "chown".to_string(),
            args: vec!["root:root".to_string(), path.to_string()],
            privileged: true,
        },
    );
    plan.push(
        format!("secure {path} permissions"),
        Risk::Low,
        Change::RunCommand {
            program: "chmod".to_string(),
            args: vec!["0440".to_string(), path.to_string()],
            privileged: true,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(IdentitySudo.name(), "identity.sudo");
    }

    fn observation(data: SudoObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn base_data() -> SudoObservation {
        SudoObservation {
            group: "superuser".to_string(),
            nis_managed: false,
            group_exists_locally: true,
            standard_sudo_rule_present: true,
            legacy_file_present: false,
            nopass_rule_current: Some("%superuser ALL=(ALL:ALL) NOPASSWD: ALL\n".to_string()),
            nopass_rule_desired: "%superuser ALL=(ALL:ALL) NOPASSWD: ALL\n".to_string(),
            nopass_rule_path: "/etc/sudoers.d/99-superuser-nopass".to_string(),
            effective_users: vec!["jallen".to_string()],
            users_missing: Vec::new(),
            users_not_in_group: Vec::new(),
        }
    }

    fn config() -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.sudo_nopass.enabled = true;
        config.sudo_nopass.group = "superuser".to_string();
        config
    }

    #[test]
    fn compliant_when_everything_matches() {
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentitySudo.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
        let plan = IdentitySudo
            .plan(&ctx, &observation(base_data()), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn missing_dropin_produces_write_plus_secure_changes() {
        let mut data = base_data();
        data.nopass_rule_current = None;
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentitySudo.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = IdentitySudo
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 3);
        assert!(matches!(plan.changes[0].change, Change::WriteFile { .. }));
        assert!(matches!(plan.changes[1].change, Change::RunCommand { .. }));
        assert!(matches!(plan.changes[2].change, Change::RunCommand { .. }));
    }

    #[test]
    fn user_not_in_group_produces_usermod() {
        let mut data = base_data();
        data.users_not_in_group = vec!["jallen".to_string()];
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentitySudo.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = IdentitySudo
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 1);
        match &plan.changes[0].change {
            Change::RunCommand { program, args, .. } => {
                assert_eq!(program, "usermod");
                assert!(args.contains(&"jallen".to_string()));
            }
            other => panic!("expected RunCommand, got {other:?}"),
        }
    }

    #[test]
    fn nis_managed_skips_group_membership_changes() {
        let mut data = base_data();
        data.nis_managed = true;
        data.group_exists_locally = false;
        data.users_not_in_group = vec!["jallen".to_string()];
        let mut config = config();
        config.sudo_nopass.nis_managed = true;
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        // group_exists_locally=false would normally trigger groupadd, but nis_managed
        // means group membership (and existence) is NIS's business, not ours.
        let diagnosis = IdentitySudo.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = IdentitySudo
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn disabled_config_is_always_compliant() {
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let mut data = base_data();
        data.nopass_rule_current = None;
        let diagnosis = IdentitySudo.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = IdentitySudo
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }
}
