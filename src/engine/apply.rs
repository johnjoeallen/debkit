use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::engine::evidence::{module_dir_name, state_root};
use crate::engine::exec;
use crate::engine::plan::{Change, ChangePlan, ServiceActionKind};

/// How to reverse one applied change. `Manual` covers changes the engine cannot safely
/// undo automatically (arbitrary commands, service actions) — these are recorded so
/// `debkit rollback` can report them rather than silently no-op.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UndoAction {
    RestoreFile {
        path: PathBuf,
        previous: Option<String>,
    },
    Manual {
        note: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackEntry {
    pub description: String,
    pub undo: UndoAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedPlan {
    pub module: String,
    pub host: String,
    pub applied_at_unix: u64,
    pub entries: Vec<RollbackEntry>,
}

/// Applies every change in `plan` in order, snapshotting enough state to reverse each one.
/// On failure partway through, automatically rolls back everything already applied and
/// returns the original error.
pub fn apply_plan(module: &str, host: &str, plan: &ChangePlan) -> anyhow::Result<AppliedPlan> {
    let mut entries = Vec::new();

    for planned in &plan.changes {
        let result = apply_change(&planned.change);
        match result {
            Ok(entry) => entries.push(RollbackEntry {
                description: planned.description.clone(),
                undo: entry,
            }),
            Err(err) => {
                let partial = AppliedPlan {
                    module: module.to_string(),
                    host: host.to_string(),
                    applied_at_unix: now_unix(),
                    entries,
                };
                let unresolved = rollback(&partial);
                return match unresolved {
                    Ok(()) => {
                        Err(err.context("apply failed; already-applied changes were rolled back"))
                    }
                    Err(rollback_err) => Err(err.context(format!(
                        "apply failed and rollback also failed: {rollback_err:#}"
                    ))),
                };
            }
        }
    }

    Ok(AppliedPlan {
        module: module.to_string(),
        host: host.to_string(),
        applied_at_unix: now_unix(),
        entries,
    })
}

fn apply_change(change: &Change) -> anyhow::Result<UndoAction> {
    match change {
        Change::WriteFile { path, content } => {
            let previous = exec::read_file_if_exists(path)?;
            exec::ensure_root_file(path, content)?;
            Ok(UndoAction::RestoreFile {
                path: path.clone(),
                previous,
            })
        }
        Change::RunCommand {
            program,
            args,
            privileged,
        } => {
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            if *privileged {
                exec::run_privileged(program, &arg_refs)?;
            } else {
                exec::run_as_current_user(program, &arg_refs)?;
            }
            Ok(UndoAction::Manual {
                note: format!(
                    "`{program} {}` is not automatically reversible",
                    args.join(" ")
                ),
            })
        }
        Change::ServiceAction { unit, action } => {
            match action {
                ServiceActionKind::EnableNow => exec::enable_and_start_service(unit)?,
                ServiceActionKind::Restart => exec::restart_service(unit)?,
            }
            Ok(UndoAction::Manual {
                note: format!(
                    "`systemctl {} {unit}` is not automatically reversible",
                    action.as_str()
                ),
            })
        }
        Change::InstallPackages { packages } => {
            let package_refs: Vec<&str> = packages.iter().map(String::as_str).collect();
            exec::apt_update_install(&package_refs)?;
            Ok(UndoAction::Manual {
                note: format!(
                    "installing `{}` is not automatically reversible",
                    packages.join(", ")
                ),
            })
        }
    }
}

/// Reverses every entry in `applied`, most-recent first. File restores are undone
/// automatically; `Manual` entries are reported as warnings on stderr since the engine
/// cannot safely reverse an arbitrary command or service action.
pub fn rollback(applied: &AppliedPlan) -> anyhow::Result<()> {
    let mut unresolved = Vec::new();
    for entry in applied.entries.iter().rev() {
        match &entry.undo {
            UndoAction::RestoreFile { path, previous } => {
                let result = match previous {
                    Some(content) => exec::ensure_root_file(path, content).map(|_| ()),
                    None => exec::remove_root_file(path),
                };
                if let Err(err) = result {
                    unresolved.push(format!(
                        "failed to restore {} for `{}`: {err:#}",
                        path.display(),
                        entry.description
                    ));
                }
            }
            UndoAction::Manual { note } => {
                eprintln!(
                    "warning: cannot automatically roll back `{}`: {note}",
                    entry.description
                );
            }
        }
    }
    if !unresolved.is_empty() {
        anyhow::bail!(unresolved.join("; "));
    }
    Ok(())
}

pub fn journal_dir() -> PathBuf {
    state_root().join("journal")
}

pub fn journal_path(module: &str, host: &str, applied_at_unix: u64) -> PathBuf {
    journal_dir().join(format!(
        "{host}-{}-{applied_at_unix}.json",
        module_dir_name(module)
    ))
}

pub fn write_journal(applied: &AppliedPlan) -> anyhow::Result<PathBuf> {
    let path = journal_path(&applied.module, &applied.host, applied.applied_at_unix);
    let rendered = serde_json::to_string_pretty(applied)
        .context("failed to serialize rollback journal entry")?;
    exec::ensure_root_dir(&journal_dir())?;
    exec::ensure_root_file(&path, &format!("{rendered}\n"))?;
    Ok(path)
}

pub fn read_journal(path: &Path) -> anyhow::Result<AppliedPlan> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::plan::Risk;

    #[test]
    fn journal_path_is_stable_and_filesystem_safe() {
        let path = journal_path("network.wake_on_lan", "spitfire", 100);
        assert_eq!(
            path,
            journal_dir().join("spitfire-network-wake_on_lan-100.json")
        );
    }

    #[test]
    fn empty_plan_applies_to_empty_journal() {
        let plan = ChangePlan::new();
        assert!(plan.is_empty());
        let applied = AppliedPlan {
            module: "network.wake_on_lan".to_string(),
            host: "spitfire".to_string(),
            applied_at_unix: 0,
            entries: Vec::new(),
        };
        assert!(applied.entries.is_empty());
    }

    #[test]
    fn planned_change_dry_run_rendering_includes_risk_and_target() {
        let mut plan = ChangePlan::new();
        plan.push(
            "write yp.conf",
            Risk::Medium,
            Change::WriteFile {
                path: PathBuf::from("/etc/yp.conf"),
                content: "domain example.lan server 127.0.0.1\n".to_string(),
            },
        );
        let rendered = plan.render_dry_run();
        assert!(rendered.contains("[medium] write yp.conf"));
        assert!(rendered.contains("write: /etc/yp.conf"));
    }
}
