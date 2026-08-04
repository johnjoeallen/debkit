use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::{Change, ChangePlan, Risk};

const PAM_DIR: &str = "/etc/pam.d";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PamServiceState {
    service: String,
    path: String,
    /// `None` when the service's PAM file doesn't exist (service not installed on this
    /// host) — nothing to manage, and not treated as a mismatch.
    current_content: Option<String>,
    desired_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PamObservation {
    skeleton: String,
    umask: String,
    services: Vec<PamServiceState>,
}

pub struct IdentityPam;

impl Module for IdentityPam {
    fn name(&self) -> &'static str {
        "identity.pam"
    }

    fn description(&self) -> &'static str {
        "pam_mkhomedir.so for create-home-on-first-login, per configured PAM service"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let config = &ctx.config.pam;
        if !config.create_home_on_first_login {
            let mut observation = Observation::new(serde_json::json!({"enabled": false}));
            observation = observation
                .with_warning("home creation on first login is disabled (`pam.create_home_on_first_login = false`)");
            return Ok(observation);
        }

        let mut services = Vec::new();
        for service in &config.services {
            let service = service.trim();
            if service.is_empty() {
                continue;
            }
            let path = format!("{PAM_DIR}/{service}");
            let current_content = fs::read_to_string(&path).ok();
            let desired_content = current_content
                .as_deref()
                .map(|current| desired_pam_content(current, &config.umask, &config.skeleton));
            services.push(PamServiceState {
                service: service.to_string(),
                path,
                current_content,
                desired_content,
            });
        }

        let data = PamObservation {
            skeleton: config.skeleton.clone(),
            umask: config.umask.clone(),
            services,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = Some("pam".to_string());
        for service in &data.services {
            if service.current_content.is_none() {
                observation = observation.with_warning(format!(
                    "{} does not exist; `{}` may not be installed on this host",
                    service.path, service.service
                ));
            }
        }
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.pam.create_home_on_first_login {
            return Diagnosis::compliant();
        }

        let data: PamObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read identity.pam observation: {err}"
                )]);
            }
        };

        let findings: Vec<String> = data
            .services
            .iter()
            .filter(|service| {
                service.current_content.is_some()
                    && service.current_content != service.desired_content
            })
            .map(|service| {
                format!(
                    "{} is missing an active pam_mkhomedir.so line",
                    service.path
                )
            })
            .collect();

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
        if !ctx.config.pam.create_home_on_first_login || diagnosis.compliant {
            return Ok(plan);
        }

        let data: PamObservation = serde_json::from_value(observation.data.clone())?;
        for service in &data.services {
            let (Some(current), Some(desired)) =
                (&service.current_content, &service.desired_content)
            else {
                continue;
            };
            if current != desired {
                plan.push(
                    format!("enable pam_mkhomedir for `{}`", service.service),
                    Risk::High,
                    Change::WriteFile {
                        path: PathBuf::from(&service.path),
                        content: desired.clone(),
                    },
                );
            }
        }
        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let config = &ctx.config.pam;
        if !config.create_home_on_first_login {
            return Ok(vec![VerificationResult::skipped(
                "identity.pam",
                "disabled in config",
            )]);
        }

        let mut checks = Vec::new();
        for service in &config.services {
            let service = service.trim();
            if service.is_empty() {
                continue;
            }
            let path = format!("{PAM_DIR}/{service}");
            let check_name = format!("{path} has an active pam_mkhomedir.so line");
            match fs::read_to_string(&path) {
                Ok(content) if has_active_mkhomedir(&content) => {
                    checks.push(VerificationResult::pass(check_name));
                }
                Ok(_) => checks.push(VerificationResult::fail(
                    check_name,
                    "no active pam_mkhomedir.so line found",
                )),
                Err(_) => checks.push(VerificationResult::skipped(
                    check_name,
                    "file does not exist",
                )),
            }
        }
        Ok(checks)
    }
}

fn has_active_mkhomedir(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with('#') && trimmed.contains("pam_mkhomedir.so")
    })
}

/// Appends a `pam_mkhomedir.so` session line if one isn't already active. Leaves the file
/// byte-for-byte unchanged when a line is already present — this is what keeps `plan()`
/// idempotent and avoids duplicating an existing entry (doc §6.4).
fn desired_pam_content(current: &str, umask: &str, skeleton: &str) -> String {
    if has_active_mkhomedir(current) {
        return current.to_string();
    }
    let mut out = current.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "session optional pam_mkhomedir.so umask={umask} skel={skeleton}\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(IdentityPam.name(), "identity.pam");
    }

    #[test]
    fn desired_content_appends_line_when_absent() {
        let current = "@include common-auth\n@include common-session\n";
        let desired = desired_pam_content(current, "0022", "/etc/skel");
        assert!(desired.starts_with(current));
        assert!(desired.contains("session optional pam_mkhomedir.so umask=0022 skel=/etc/skel"));
    }

    #[test]
    fn desired_content_is_unchanged_when_already_present() {
        let current = "@include common-session\nsession optional pam_mkhomedir.so umask=0022 skel=/etc/skel\n";
        assert_eq!(desired_pam_content(current, "0022", "/etc/skel"), current);
    }

    #[test]
    fn commented_mkhomedir_line_is_not_active() {
        assert!(!has_active_mkhomedir(
            "# session optional pam_mkhomedir.so\n"
        ));
    }

    fn observation(data: PamObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn config() -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.pam.create_home_on_first_login = true;
        config
    }

    #[test]
    fn compliant_when_all_services_already_have_the_line() {
        let content = "@include common-session\nsession optional pam_mkhomedir.so umask=0022 skel=/etc/skel\n";
        let data = PamObservation {
            skeleton: "/etc/skel".to_string(),
            umask: "0022".to_string(),
            services: vec![PamServiceState {
                service: "login".to_string(),
                path: "/etc/pam.d/login".to_string(),
                current_content: Some(content.to_string()),
                desired_content: Some(content.to_string()),
            }],
        };
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentityPam.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = IdentityPam
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn missing_line_produces_write_change() {
        let current = "@include common-session\n".to_string();
        let desired = desired_pam_content(&current, "0022", "/etc/skel");
        let data = PamObservation {
            skeleton: "/etc/skel".to_string(),
            umask: "0022".to_string(),
            services: vec![PamServiceState {
                service: "sshd".to_string(),
                path: "/etc/pam.d/sshd".to_string(),
                current_content: Some(current),
                desired_content: Some(desired),
            }],
        };
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentityPam.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = IdentityPam
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 1);
        match &plan.changes[0].change {
            Change::WriteFile { path, content } => {
                assert_eq!(path, &PathBuf::from("/etc/pam.d/sshd"));
                assert!(content.contains("pam_mkhomedir.so"));
            }
            other => panic!("expected WriteFile, got {other:?}"),
        }
    }

    #[test]
    fn missing_service_file_is_not_a_mismatch() {
        let data = PamObservation {
            skeleton: "/etc/skel".to_string(),
            umask: "0022".to_string(),
            services: vec![PamServiceState {
                service: "gdm-password".to_string(),
                path: "/etc/pam.d/gdm-password".to_string(),
                current_content: None,
                desired_content: None,
            }],
        };
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = IdentityPam.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = IdentityPam
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
        let data = PamObservation {
            skeleton: "/etc/skel".to_string(),
            umask: "0022".to_string(),
            services: vec![PamServiceState {
                service: "login".to_string(),
                path: "/etc/pam.d/login".to_string(),
                current_content: Some("garbage".to_string()),
                desired_content: Some("garbage with mkhomedir".to_string()),
            }],
        };
        let diagnosis = IdentityPam.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = IdentityPam
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }
}
