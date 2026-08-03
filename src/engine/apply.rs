use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::engine::evidence::{module_dir_name, state_root};
use crate::engine::exec;
use crate::engine::plan::{Change, ChangePlan, ServiceActionKind};

/// How to reverse one applied change. `Manual` covers changes the engine cannot safely
/// undo automatically (arbitrary commands, service restarts) — these are recorded so
/// `debkit rollback` can report them rather than silently no-op.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UndoAction {
    RestoreFile {
        path: PathBuf,
        previous: Option<String>,
    },
    /// Reverses `ServiceAction::EnableNow` by restoring whichever of enabled/active
    /// state wasn't already true beforehand. `ServiceAction::Restart` has no
    /// meaningful undo — there's nothing to "restore" about a restart once it's
    /// happened — and stays `Manual`.
    RestoreServiceState {
        unit: String,
        was_enabled: bool,
        was_active: bool,
    },
    /// Reverses `InstallPackages` by removing only the packages that were captured as
    /// *not* already installed before `apply` ran — never a package the user already
    /// had for unrelated reasons. Empty when every declared package was already
    /// present.
    RemovePackages {
        packages: Vec<String>,
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
        Change::ServiceAction { unit, action } => match action {
            ServiceActionKind::EnableNow => {
                let was_enabled = exec::systemctl_is_enabled(unit);
                let was_active = exec::systemctl_is_active(unit);
                exec::enable_and_start_service(unit)?;
                Ok(UndoAction::RestoreServiceState {
                    unit: unit.clone(),
                    was_enabled,
                    was_active,
                })
            }
            ServiceActionKind::Restart => {
                exec::restart_service(unit)?;
                Ok(UndoAction::Manual {
                    note: format!("`systemctl restart {unit}` is not automatically reversible"),
                })
            }
        },
        Change::InstallPackages { packages } => {
            let mut newly_installed = Vec::new();
            for package in packages {
                if !exec::package_installed(package)? {
                    newly_installed.push(package.clone());
                }
            }
            let package_refs: Vec<&str> = packages.iter().map(String::as_str).collect();
            exec::apt_update_install(&package_refs)?;
            Ok(UndoAction::RemovePackages {
                packages: newly_installed,
            })
        }
    }
}

/// Reverses every entry in `applied`, most-recent first. File restores, service
/// enable/start, and package installs are all undone automatically; `Manual` entries
/// (arbitrary `RunCommand`s, service restarts) are reported as warnings on stderr since
/// the engine cannot safely reverse them.
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
            UndoAction::RestoreServiceState {
                unit,
                was_enabled,
                was_active,
            } => {
                if !was_active && let Err(err) = exec::stop_service(unit) {
                    unresolved.push(format!(
                        "failed to stop {unit} for `{}`: {err:#}",
                        entry.description
                    ));
                }
                if !was_enabled && let Err(err) = exec::disable_service(unit) {
                    unresolved.push(format!(
                        "failed to disable {unit} for `{}`: {err:#}",
                        entry.description
                    ));
                }
            }
            UndoAction::RemovePackages { packages } => {
                if !packages.is_empty() {
                    let package_refs: Vec<&str> = packages.iter().map(String::as_str).collect();
                    if let Err(err) = exec::apt_remove(&package_refs) {
                        unresolved.push(format!(
                            "failed to remove {} for `{}`: {err:#}",
                            packages.join(", "),
                            entry.description
                        ));
                    }
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

/// Finds the most recently written journal entry for `module` on `host`, for
/// `debkit rollback <module>` without requiring the caller to know the exact filename
/// `apply` printed.
pub fn find_latest_journal(module: &str, host: &str) -> anyhow::Result<Option<PathBuf>> {
    let dir = journal_dir();
    if !dir.is_dir() {
        return Ok(None);
    }
    let prefix = format!("{host}-{}-", module_dir_name(module));
    let mut candidates: Vec<(u64, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(timestamp) = journal_timestamp(&prefix, &name) {
            candidates.push((timestamp, entry.path()));
        }
    }
    Ok(candidates
        .into_iter()
        .max_by_key(|(timestamp, _)| *timestamp)
        .map(|(_, path)| path))
}

/// Parses the timestamp out of a journal filename matching `<prefix><timestamp>.json`,
/// pulled out of `find_latest_journal` so the parsing logic is testable without touching
/// the filesystem or `DEBKIT_STATE_DIR`.
fn journal_timestamp(prefix: &str, filename: &str) -> Option<u64> {
    filename
        .strip_prefix(prefix)?
        .strip_suffix(".json")?
        .parse::<u64>()
        .ok()
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
    fn restore_service_state_round_trips_through_json() {
        let undo = UndoAction::RestoreServiceState {
            unit: "dnsmasq".to_string(),
            was_enabled: false,
            was_active: true,
        };
        let rendered = serde_json::to_string(&undo).unwrap();
        let parsed: UndoAction = serde_json::from_str(&rendered).unwrap();
        match parsed {
            UndoAction::RestoreServiceState {
                unit,
                was_enabled,
                was_active,
            } => {
                assert_eq!(unit, "dnsmasq");
                assert!(!was_enabled);
                assert!(was_active);
            }
            other => panic!("expected RestoreServiceState, got {other:?}"),
        }
    }

    #[test]
    fn remove_packages_round_trips_through_json() {
        let undo = UndoAction::RemovePackages {
            packages: vec!["dnsmasq".to_string()],
        };
        let rendered = serde_json::to_string(&undo).unwrap();
        let parsed: UndoAction = serde_json::from_str(&rendered).unwrap();
        match parsed {
            UndoAction::RemovePackages { packages } => assert_eq!(packages, vec!["dnsmasq"]),
            other => panic!("expected RemovePackages, got {other:?}"),
        }
    }

    #[test]
    fn journal_timestamp_parses_matching_filenames() {
        let prefix = "spitfire-network-wake_on_lan-";
        assert_eq!(
            journal_timestamp(prefix, "spitfire-network-wake_on_lan-100.json"),
            Some(100)
        );
        assert_eq!(
            journal_timestamp(prefix, "spitfire-identity-nis-100.json"),
            None
        );
        assert_eq!(
            journal_timestamp(prefix, "spitfire-network-wake_on_lan-notanumber.json"),
            None
        );
    }

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
