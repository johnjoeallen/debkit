use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::{Change, ChangePlan, Risk};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HelperEntry {
    origin: String,
    value: String,
    /// "global" when the origin resolves to the invoking user's `~/.gitconfig` or
    /// `~/.config/git/config` — the only scope this module manages. Anything else
    /// (system `/etc/gitconfig`, a local repo's `.git/config`) is surfaced as a finding
    /// but left alone, since fixing it needs a different privilege level or isn't this
    /// module's business.
    scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GitObservation {
    git_installed: bool,
    git_version: Option<String>,
    helper_entries: Vec<HelperEntry>,
    desired_helper_value: String,
    store_file: Option<String>,
    store_file_exists: bool,
    store_file_mode_octal: Option<String>,
    user_name: Option<String>,
    user_email: Option<String>,
}

pub struct DeveloperGit;

impl Module for DeveloperGit {
    fn name(&self) -> &'static str {
        "developer.git"
    }

    fn config_key(&self) -> Option<&'static str> {
        Some("git")
    }

    fn description(&self) -> &'static str {
        "global git credential.helper and credential-store file permissions"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let config = &ctx.config.git;
        let git_installed = exec::command_available("git");
        let git_version = git_installed
            .then(|| exec::capture("git", &["--version"]).ok())
            .flatten()
            .map(|raw| raw.trim().to_string());

        let home = crate::config::home_dir().ok();
        let helper_entries = read_credential_helper_entries(home.as_deref());
        let desired_helper_value = desired_helper_value(config);
        let store_file = resolve_store_file(config, home.as_deref());
        let store_file_exists = store_file.as_ref().is_some_and(|path| path.exists());
        let store_file_mode_octal = store_file
            .as_ref()
            .filter(|_| store_file_exists)
            .and_then(|path| file_mode_octal(path));

        let user_name = exec::capture("git", &["config", "--global", "user.name"])
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|value| !value.is_empty());
        let user_email = exec::capture("git", &["config", "--global", "user.email"])
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|value| !value.is_empty());

        let data = GitObservation {
            git_installed,
            git_version,
            helper_entries,
            desired_helper_value,
            store_file: store_file.map(|path| path.display().to_string()),
            store_file_exists,
            store_file_mode_octal,
            user_name,
            user_email,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = Some("git".to_string());
        if !git_installed {
            observation = observation.with_warning("git is not installed");
        }
        let non_global: Vec<&HelperEntry> = data
            .helper_entries
            .iter()
            .filter(|entry| entry.scope != "global")
            .collect();
        if !non_global.is_empty() {
            observation = observation.with_warning(format!(
                "credential.helper is also set outside the scope this module manages ({} entr{})",
                non_global.len(),
                if non_global.len() == 1 { "y" } else { "ies" }
            ));
        }
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.git.enabled {
            return Diagnosis::compliant();
        }

        let data: GitObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read developer.git observation: {err}"
                )]);
            }
        };

        let mut findings = Vec::new();
        if !data.git_installed {
            findings.push("git is not installed".to_string());
        }

        let global_entries: Vec<&HelperEntry> = data
            .helper_entries
            .iter()
            .filter(|entry| entry.scope == "global")
            .collect();
        let desired = &data.desired_helper_value;
        if desired.is_empty() {
            if !global_entries.is_empty() {
                findings.push(format!(
                    "expected no global credential.helper, found: {}",
                    global_entries
                        .iter()
                        .map(|entry| entry.value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        } else {
            let matches_desired = global_entries.len() == 1 && global_entries[0].value == *desired;
            if !matches_desired {
                findings.push(format!(
                    "global credential.helper should be exactly `{desired}` (found: {})",
                    if global_entries.is_empty() {
                        "none".to_string()
                    } else {
                        global_entries
                            .iter()
                            .map(|entry| entry.value.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ));
            }
        }

        let non_global: Vec<&HelperEntry> = data
            .helper_entries
            .iter()
            .filter(|entry| entry.scope != "global")
            .collect();
        if !non_global.is_empty() {
            findings.push(format!(
                "credential.helper is also set outside the global scope this module manages: {}",
                non_global
                    .iter()
                    .map(|entry| format!("{}={}", entry.origin, entry.value))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }

        if let Some(mode) = &data.store_file_mode_octal
            && mode != "600"
        {
            findings.push(format!(
                "{} has mode {mode}, expected 600",
                data.store_file
                    .as_deref()
                    .unwrap_or("credential store file")
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
        if !ctx.config.git.enabled || diagnosis.compliant {
            return Ok(plan);
        }

        let data: GitObservation = serde_json::from_value(observation.data.clone())?;
        let global_entries: Vec<&HelperEntry> = data
            .helper_entries
            .iter()
            .filter(|entry| entry.scope == "global")
            .collect();
        let desired = &data.desired_helper_value;
        let already_exact =
            !desired.is_empty() && global_entries.len() == 1 && global_entries[0].value == *desired;
        let already_empty = desired.is_empty() && global_entries.is_empty();

        if !already_exact && !already_empty {
            if !global_entries.is_empty() {
                plan.push(
                    "clear existing global credential.helper entries",
                    Risk::Medium,
                    Change::RunCommand {
                        program: "git".to_string(),
                        args: vec![
                            "config".to_string(),
                            "--global".to_string(),
                            "--unset-all".to_string(),
                            "credential.helper".to_string(),
                        ],
                        privileged: false,
                    },
                );
            }
            if !desired.is_empty() {
                plan.push(
                    format!("set global credential.helper to `{desired}`"),
                    Risk::Low,
                    Change::RunCommand {
                        program: "git".to_string(),
                        args: vec![
                            "config".to_string(),
                            "--global".to_string(),
                            "--add".to_string(),
                            "credential.helper".to_string(),
                            desired.clone(),
                        ],
                        privileged: false,
                    },
                );
            }
        }

        if let (Some(mode), Some(path)) = (&data.store_file_mode_octal, &data.store_file)
            && mode != "600"
        {
            plan.push(
                format!("restrict permissions on {path} to 600"),
                Risk::Medium,
                Change::RunCommand {
                    program: "chmod".to_string(),
                    args: vec!["600".to_string(), path.clone()],
                    privileged: false,
                },
            );
        }

        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let config = &ctx.config.git;
        if !config.enabled {
            return Ok(vec![VerificationResult::skipped(
                "developer.git",
                "disabled in config",
            )]);
        }

        let home = crate::config::home_dir().ok();
        let entries = read_credential_helper_entries(home.as_deref());
        let global: Vec<&HelperEntry> = entries
            .iter()
            .filter(|entry| entry.scope == "global")
            .collect();
        let desired = desired_helper_value(config);

        let mut checks = Vec::new();
        let helper_ok = if desired.is_empty() {
            global.is_empty()
        } else {
            global.len() == 1 && global[0].value == desired
        };
        checks.push(if helper_ok {
            VerificationResult::pass("global credential.helper matches declared intent")
        } else {
            VerificationResult::fail(
                "global credential.helper matches declared intent",
                format!(
                    "found: {}",
                    global
                        .iter()
                        .map(|entry| entry.value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        });

        if let Some(path) = resolve_store_file(config, home.as_deref())
            && path.exists()
        {
            checks.push(match file_mode_octal(&path) {
                Some(mode) if mode == "600" => {
                    VerificationResult::pass(format!("{} has mode 600", path.display()))
                }
                Some(mode) => VerificationResult::fail(
                    format!("{} has mode 600", path.display()),
                    format!("found mode {mode}"),
                ),
                None => VerificationResult::skipped(
                    format!("{} has mode 600", path.display()),
                    "could not read file mode",
                ),
            });
        }

        Ok(checks)
    }
}

fn desired_helper_value(config: &crate::config::GitConfig) -> String {
    match config.credential_helper.as_str() {
        "store" => {
            let file = config.credential_store_file.trim();
            if file.is_empty() {
                "store".to_string()
            } else {
                format!("store --file={file}")
            }
        }
        "none" => String::new(),
        other => other.to_string(),
    }
}

fn resolve_store_file(config: &crate::config::GitConfig, home: Option<&Path>) -> Option<PathBuf> {
    if config.credential_helper != "store" {
        return None;
    }
    let raw = config.credential_store_file.trim();
    let raw = if raw.is_empty() {
        "~/.git-credentials"
    } else {
        raw
    };
    if let Some(rest) = raw.strip_prefix("~/") {
        home.map(|home| home.join(rest))
    } else {
        Some(PathBuf::from(raw))
    }
}

fn read_credential_helper_entries(home: Option<&Path>) -> Vec<HelperEntry> {
    let Ok(raw) = exec::capture(
        "git",
        &["config", "--show-origin", "--get-all", "credential.helper"],
    ) else {
        return Vec::new();
    };
    let global_origins: Vec<String> = home
        .map(|home| {
            vec![
                format!("file:{}", home.join(".gitconfig").display()),
                format!("file:{}", home.join(".config/git/config").display()),
            ]
        })
        .unwrap_or_default();

    raw.lines()
        .filter_map(|line| {
            let (origin, value) = line.split_once('\t')?;
            let scope = if global_origins.iter().any(|candidate| candidate == origin) {
                "global"
            } else {
                "other"
            };
            Some(HelperEntry {
                origin: origin.to_string(),
                value: value.to_string(),
                scope: scope.to_string(),
            })
        })
        .collect()
}

fn file_mode_octal(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .map(|metadata| format!("{:o}", metadata.permissions().mode() & 0o777))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(DeveloperGit.name(), "developer.git");
    }

    fn observation(data: GitObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn base_data() -> GitObservation {
        GitObservation {
            git_installed: true,
            git_version: Some("git version 2.45.0".to_string()),
            helper_entries: vec![HelperEntry {
                origin: "file:/home/jallen/.gitconfig".to_string(),
                value: "store".to_string(),
                scope: "global".to_string(),
            }],
            desired_helper_value: "store".to_string(),
            store_file: Some("/home/jallen/.git-credentials".to_string()),
            store_file_exists: true,
            store_file_mode_octal: Some("600".to_string()),
            user_name: Some("John Allen".to_string()),
            user_email: Some("john@example.com".to_string()),
        }
    }

    fn config() -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.git.enabled = true;
        config
    }

    #[test]
    fn compliant_when_exactly_one_matching_global_helper() {
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = DeveloperGit.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
        let plan = DeveloperGit
            .plan(&ctx, &observation(base_data()), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn conflicting_helpers_produce_clear_then_set() {
        let mut data = base_data();
        data.helper_entries = vec![
            HelperEntry {
                origin: "file:/home/jallen/.gitconfig".to_string(),
                value: "cache".to_string(),
                scope: "global".to_string(),
            },
            HelperEntry {
                origin: "file:/home/jallen/.gitconfig".to_string(),
                value: "store".to_string(),
                scope: "global".to_string(),
            },
        ];
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = DeveloperGit.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = DeveloperGit
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 2);
        match &plan.changes[0].change {
            Change::RunCommand {
                args, privileged, ..
            } => {
                assert!(args.contains(&"--unset-all".to_string()));
                assert!(!privileged, "git config --global must not run via sudo");
            }
            other => panic!("expected RunCommand, got {other:?}"),
        }
    }

    #[test]
    fn insecure_store_file_mode_is_fixed() {
        let mut data = base_data();
        data.store_file_mode_octal = Some("644".to_string());
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = DeveloperGit.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = DeveloperGit
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 1);
        match &plan.changes[0].change {
            Change::RunCommand {
                program,
                args,
                privileged,
            } => {
                assert_eq!(program, "chmod");
                assert_eq!(args[0], "600");
                assert!(!privileged);
            }
            other => panic!("expected RunCommand, got {other:?}"),
        }
    }

    #[test]
    fn non_global_helper_is_flagged_but_not_touched() {
        let mut data = base_data();
        data.helper_entries.push(HelperEntry {
            origin: "file:/etc/gitconfig".to_string(),
            value: "some-system-helper".to_string(),
            scope: "other".to_string(),
        });
        let config = config();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let diagnosis = DeveloperGit.diagnose(&ctx, &observation(data));
        assert!(!diagnosis.compliant);
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|finding| finding.contains("/etc/gitconfig"))
        );
    }

    #[test]
    fn disabled_config_is_always_compliant() {
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "spitfire".to_string(),
            config: &config,
        };
        let mut data = base_data();
        data.helper_entries.clear();
        let diagnosis = DeveloperGit.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = DeveloperGit
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn desired_value_none_means_empty_string() {
        let mut config = crate::config::GitConfig::default();
        config.credential_helper = "none".to_string();
        assert_eq!(desired_helper_value(&config), "");
    }

    #[test]
    fn desired_value_store_with_custom_file() {
        let mut config = crate::config::GitConfig::default();
        config.credential_helper = "store".to_string();
        config.credential_store_file = "/etc/debkit/git-credentials".to_string();
        assert_eq!(
            desired_helper_value(&config),
            "store --file=/etc/debkit/git-credentials"
        );
    }

    #[test]
    fn resolve_store_file_expands_home_tilde() {
        let config = crate::config::GitConfig::default();
        let home = Path::new("/home/jallen");
        assert_eq!(
            resolve_store_file(&config, Some(home)),
            Some(PathBuf::from("/home/jallen/.git-credentials"))
        );
    }
}
