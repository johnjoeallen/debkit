use std::fs;
use std::path::PathBuf;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::engine::plan::{ChangePlan, Risk};

pub const EVIDENCE_ROOT: &str = "/var/lib/debkit";

/// `/var/lib/debkit` by default; overridable via `DEBKIT_STATE_DIR` so tests and dry runs
/// never touch the real host's evidence/journal store.
pub fn state_root() -> PathBuf {
    std::env::var_os("DEBKIT_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(EVIDENCE_ROOT))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// discover()+diagnose() found no mismatch; plan() would be empty.
    Compliant,
    /// apply() ran a non-empty plan and verify() passed afterward.
    Changed,
    /// diagnose() found more than one active owner for the same resource.
    Conflict,
    /// discover(), plan(), apply(), or verify() failed.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckResult {
    Pass,
    Fail,
    Skipped,
}

/// One functional verification outcome, e.g. a `dig` lookup or a `getent` check —
/// never just a service's `is-active` state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub check: String,
    pub result: CheckResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl VerificationResult {
    pub fn pass(check: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            result: CheckResult::Pass,
            detail: None,
        }
    }

    pub fn fail(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            result: CheckResult::Fail,
            detail: Some(detail.into()),
        }
    }

    pub fn skipped(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            result: CheckResult::Skipped,
            detail: Some(detail.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedChangeSummary {
    pub description: String,
    pub risk: Risk,
}

impl From<&ChangePlan> for Vec<AppliedChangeSummary> {
    fn from(plan: &ChangePlan) -> Self {
        plan.changes
            .iter()
            .map(|planned| AppliedChangeSummary {
                description: planned.description.clone(),
                risk: planned.risk,
            })
            .collect()
    }
}

/// Structured evidence written after every `discover`/`plan`/`apply`/`verify` run, matching
/// the schema in the DebKit v2 requirements doc §4.3. Consumable by TimeVault or another
/// local orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub module: String,
    pub host: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub observed: serde_json::Value,
    pub changes: Vec<AppliedChangeSummary>,
    pub verification: Vec<VerificationResult>,
    pub artifacts: Vec<String>,
}

/// Slugifies a module name like `network.wake_on_lan` into a filesystem-safe directory
/// component (`network-wake_on_lan`) for `/var/lib/debkit/<module>/<host>.json`.
pub fn module_dir_name(module: &str) -> String {
    module.replace('.', "-")
}

pub fn evidence_path(module: &str, host: &str) -> PathBuf {
    state_root()
        .join(module_dir_name(module))
        .join(format!("{host}.json"))
}

pub fn write_evidence(evidence: &Evidence) -> anyhow::Result<PathBuf> {
    let path = evidence_path(&evidence.module, &evidence.host);
    let rendered =
        serde_json::to_string_pretty(evidence).context("failed to serialize evidence")?;
    crate::engine::exec::ensure_root_dir(path.parent().expect("evidence path has a parent"))?;
    crate::engine::exec::ensure_root_file(&path, &format!("{rendered}\n"))?;
    Ok(path)
}

pub fn read_evidence(module: &str, host: &str) -> anyhow::Result<Option<Evidence>> {
    let path = evidence_path(module, host);
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(Some(serde_json::from_str(&raw).with_context(|| {
        format!("failed to parse {}", path.display())
    })?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_dir_name_replaces_dots_with_dashes() {
        assert_eq!(
            module_dir_name("network.wake_on_lan"),
            "network-wake_on_lan"
        );
    }

    #[test]
    fn evidence_round_trips_through_json() {
        let evidence = Evidence {
            module: "network.wake_on_lan".to_string(),
            host: "spitfire".to_string(),
            status: Status::Compliant,
            owner: Some("NetworkManager".to_string()),
            observed: serde_json::json!({"interface": "lan0"}),
            changes: Vec::new(),
            verification: vec![VerificationResult::pass("persistent-profile")],
            artifacts: vec!["/var/lib/debkit/wake-on-lan/spitfire.json".to_string()],
        };
        let rendered = serde_json::to_string(&evidence).unwrap();
        assert!(rendered.contains("\"status\":\"compliant\""));
        assert!(rendered.contains("\"result\":\"pass\""));
        let parsed: Evidence = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed.module, evidence.module);
        assert_eq!(parsed.owner.as_deref(), Some("NetworkManager"));
    }
}
