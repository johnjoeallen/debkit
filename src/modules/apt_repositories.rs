use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::{Change, ChangePlan, Risk};

const PROXY_CONF_PATH: &str = "/etc/apt/apt.conf.d/01debkit-proxy";
const EXCEPTIONS_CONF_PATH: &str = "/etc/apt/apt.conf.d/02debkit-proxy-exceptions";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AptObservation {
    proxy_current: Option<String>,
    proxy_desired: Option<String>,
    exceptions_current: Option<String>,
    exceptions_desired: Option<String>,
    /// Raw `Acquire::http(s)::Proxy[::host] value` lines from `apt-config dump` — the
    /// *effective* merged proxy config, which is what actually governs apt's behavior
    /// regardless of which conf.d file set it.
    effective_proxy_settings: Vec<String>,
}

pub struct AptRepositories;

impl Module for AptRepositories {
    fn name(&self) -> &'static str {
        "apt.repositories"
    }

    fn config_key(&self) -> Option<&'static str> {
        Some("apt")
    }

    fn description(&self) -> &'static str {
        "apt-cacher-ng proxy config and DIRECT-bypass exceptions"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let config = &ctx.config.apt;

        let proxy_current = fs::read_to_string(PROXY_CONF_PATH).ok();
        let exceptions_current = fs::read_to_string(EXCEPTIONS_CONF_PATH).ok();
        let proxy_desired = desired_proxy_content(&config.proxy);
        let exceptions_desired = desired_exceptions_content(&config.direct_hosts);
        let effective_proxy_settings = read_effective_proxy_settings()
            .into_iter()
            .map(|(key, value)| format!("{key} {value}"))
            .collect();

        let data = AptObservation {
            proxy_current,
            proxy_desired,
            exceptions_current,
            exceptions_desired,
            effective_proxy_settings,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        observation.owner = Some("apt".to_string());
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.apt.enabled {
            return Diagnosis::compliant();
        }

        let data: AptObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read apt.repositories observation: {err}"
                )]);
            }
        };

        let mut findings = Vec::new();
        if data.proxy_current != data.proxy_desired {
            findings.push(format!(
                "{PROXY_CONF_PATH} does not match declared proxy intent"
            ));
        }
        if data.exceptions_current != data.exceptions_desired {
            findings.push(format!(
                "{EXCEPTIONS_CONF_PATH} does not match declared direct-host exceptions"
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
        if !ctx.config.apt.enabled || diagnosis.compliant {
            return Ok(plan);
        }

        let data: AptObservation = serde_json::from_value(observation.data.clone())?;

        match &data.proxy_desired {
            Some(content) if data.proxy_current.as_deref() != Some(content.as_str()) => {
                plan.push(
                    "configure apt proxy",
                    Risk::Medium,
                    Change::WriteFile {
                        path: PathBuf::from(PROXY_CONF_PATH),
                        content: content.clone(),
                    },
                );
            }
            None if data.proxy_current.is_some() => {
                plan.push(
                    "remove apt proxy configuration",
                    Risk::Low,
                    Change::RunCommand {
                        program: "rm".to_string(),
                        args: vec!["-f".to_string(), PROXY_CONF_PATH.to_string()],
                        privileged: true,
                    },
                );
            }
            _ => {}
        }

        match &data.exceptions_desired {
            Some(content) if data.exceptions_current.as_deref() != Some(content.as_str()) => {
                plan.push(
                    "configure apt proxy DIRECT exceptions",
                    Risk::Medium,
                    Change::WriteFile {
                        path: PathBuf::from(EXCEPTIONS_CONF_PATH),
                        content: content.clone(),
                    },
                );
            }
            None if data.exceptions_current.is_some() => {
                plan.push(
                    "remove apt proxy exceptions",
                    Risk::Low,
                    Change::RunCommand {
                        program: "rm".to_string(),
                        args: vec!["-f".to_string(), EXCEPTIONS_CONF_PATH.to_string()],
                        privileged: true,
                    },
                );
            }
            _ => {}
        }

        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let config = &ctx.config.apt;
        if !config.enabled {
            return Ok(vec![VerificationResult::skipped(
                "apt.repositories",
                "disabled in config",
            )]);
        }

        let mut checks = Vec::new();
        let effective = read_effective_proxy_settings();
        let desired_proxy = config.proxy.trim();
        let global_ok = if desired_proxy.is_empty() {
            !effective
                .iter()
                .any(|(key, _)| key == "Acquire::http::Proxy")
        } else {
            effective
                .iter()
                .any(|(key, value)| key == "Acquire::http::Proxy" && value == desired_proxy)
        };
        checks.push(if global_ok {
            VerificationResult::pass("effective apt proxy matches declared intent")
        } else {
            VerificationResult::fail(
                "effective apt proxy matches declared intent",
                format!("apt-config dump reported: {effective:?}"),
            )
        });

        if exec::command_available("curl") {
            for host in &config.direct_hosts {
                let host = host.trim();
                if host.is_empty() {
                    continue;
                }
                let url = format!("https://{host}");
                let check_name = format!("{host} reachable directly (bypassing proxy)");
                match exec::capture(
                    "curl",
                    &[
                        "-sS",
                        "-o",
                        "/dev/null",
                        "-w",
                        "%{http_code}",
                        "--max-time",
                        "5",
                        "--noproxy",
                        "*",
                        &url,
                    ],
                ) {
                    Ok(code) if code.trim().starts_with(['2', '3']) => {
                        checks.push(VerificationResult::pass(check_name));
                    }
                    Ok(code) => {
                        checks.push(VerificationResult::fail(
                            check_name,
                            format!("HTTP {}", code.trim()),
                        ));
                    }
                    Err(err) => checks.push(VerificationResult::fail(check_name, err.to_string())),
                }
            }
        } else if !config.direct_hosts.is_empty() {
            checks.push(VerificationResult::skipped(
                "direct-host connectivity",
                "curl is not installed",
            ));
        }

        Ok(checks)
    }
}

fn desired_proxy_content(proxy: &str) -> Option<String> {
    let proxy = proxy.trim();
    if proxy.is_empty() {
        None
    } else {
        Some(format!(
            "Acquire::http::Proxy \"{proxy}\";\nAcquire::https::Proxy \"{proxy}\";\n"
        ))
    }
}

fn desired_exceptions_content(hosts: &[String]) -> Option<String> {
    let mut out = String::new();
    for host in hosts {
        let host = host.trim();
        if host.is_empty() {
            continue;
        }
        out.push_str(&format!("Acquire::http::Proxy::{host} \"DIRECT\";\n"));
        out.push_str(&format!("Acquire::https::Proxy::{host} \"DIRECT\";\n"));
    }
    if out.is_empty() { None } else { Some(out) }
}

fn read_effective_proxy_settings() -> Vec<(String, String)> {
    let Ok(raw) = exec::capture("apt-config", &["dump"]) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with("Acquire::http::Proxy")
                && !line.starts_with("Acquire::https::Proxy")
            {
                return None;
            }
            let (key, rest) = line.split_once(' ')?;
            let value = rest.trim_end_matches(';').trim_matches('"').to_string();
            Some((key.to_string(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(AptRepositories.name(), "apt.repositories");
    }

    fn observation(data: AptObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn config() -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.apt.enabled = true;
        config.apt.proxy = "http://10.0.0.1:3142".to_string();
        config.apt.direct_hosts = vec!["pkgs.tailscale.com".to_string()];
        config
    }

    #[test]
    fn desired_content_renders_expected_apt_conf_syntax() {
        let proxy = desired_proxy_content("http://10.0.0.1:3142").unwrap();
        assert!(proxy.contains("Acquire::http::Proxy \"http://10.0.0.1:3142\";"));
        assert!(proxy.contains("Acquire::https::Proxy \"http://10.0.0.1:3142\";"));

        let exceptions = desired_exceptions_content(&["pkgs.tailscale.com".to_string()]).unwrap();
        assert!(exceptions.contains("Acquire::http::Proxy::pkgs.tailscale.com \"DIRECT\";"));
    }

    #[test]
    fn empty_proxy_and_hosts_desire_no_files() {
        assert_eq!(desired_proxy_content(""), None);
        assert_eq!(desired_exceptions_content(&[]), None);
    }

    #[test]
    fn compliant_when_current_matches_desired() {
        let config = config();
        let data = AptObservation {
            proxy_current: desired_proxy_content(&config.apt.proxy),
            proxy_desired: desired_proxy_content(&config.apt.proxy),
            exceptions_current: desired_exceptions_content(&config.apt.direct_hosts),
            exceptions_desired: desired_exceptions_content(&config.apt.direct_hosts),
            effective_proxy_settings: Vec::new(),
        };
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let diagnosis = AptRepositories.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = AptRepositories
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn missing_proxy_file_produces_write_change() {
        let config = config();
        let data = AptObservation {
            proxy_current: None,
            proxy_desired: desired_proxy_content(&config.apt.proxy),
            exceptions_current: None,
            exceptions_desired: desired_exceptions_content(&config.apt.direct_hosts),
            effective_proxy_settings: Vec::new(),
        };
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let diagnosis = AptRepositories.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = AptRepositories
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 2);
        assert!(matches!(plan.changes[0].change, Change::WriteFile { .. }));
        assert!(matches!(plan.changes[1].change, Change::WriteFile { .. }));
    }

    #[test]
    fn stale_files_are_removed_when_desired_is_none() {
        let mut config = crate::config::DebkitConfig::default();
        config.apt.enabled = true;
        let data = AptObservation {
            proxy_current: Some("Acquire::http::Proxy \"http://old:3142\";\n".to_string()),
            proxy_desired: None,
            exceptions_current: Some("Acquire::http::Proxy::old.example \"DIRECT\";\n".to_string()),
            exceptions_desired: None,
            effective_proxy_settings: Vec::new(),
        };
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let diagnosis = AptRepositories.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = AptRepositories
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 2);
        for planned in &plan.changes {
            match &planned.change {
                Change::RunCommand {
                    program,
                    privileged,
                    ..
                } => {
                    assert_eq!(program, "rm");
                    assert!(privileged, "removing /etc/apt files needs root");
                }
                other => panic!("expected RunCommand, got {other:?}"),
            }
        }
    }

    #[test]
    fn disabled_config_is_always_compliant() {
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let data = AptObservation {
            proxy_current: Some("garbage".to_string()),
            proxy_desired: None,
            exceptions_current: None,
            exceptions_desired: None,
            effective_proxy_settings: Vec::new(),
        };
        let diagnosis = AptRepositories.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = AptRepositories
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }
}
