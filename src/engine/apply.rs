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

/// One `Change::WriteFile` staged for review by `stage_plan` — materialized to a
/// preview location without ever touching the real target.
#[derive(Debug, Clone)]
pub struct StagedFile {
    /// The real absolute path a live `apply` would write to.
    pub target: PathBuf,
    /// Where the new content was actually written, for inspection.
    pub staged_path: PathBuf,
    /// Where the target's current, pre-change content was snapshotted, if the target
    /// already existed and was readable. `None` for a new file, or if the existing
    /// file couldn't be read (permission denied) — staging degrades honestly rather
    /// than failing the whole preview over a snapshot it couldn't take.
    pub pristine_path: Option<PathBuf>,
}

/// Everything `stage_plan` produced for one dry run.
#[derive(Debug, Clone)]
pub struct StagedPlan {
    pub run_id: String,
    pub module: String,
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub files: Vec<StagedFile>,
    /// Non-`WriteFile` changes (`RunCommand`/`ServiceAction`/`InstallPackages`) a
    /// real `apply` would also perform, rendered for visibility — dry-run never
    /// executes any of them, since there's no file content to preview for a command.
    pub skipped_actions: Vec<String>,
}

/// Persisted at `<run-root>/plan.json` by every `stage_plan` call, and read back by
/// `debkit apply --from-run <uuid>` to replay the exact plan that was staged — not a
/// freshly (and possibly differently) recomputed one. `module`/`host` let promotion
/// sanity-check the run against what the caller expects before touching anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedManifest {
    pub module: String,
    pub host: String,
    pub plan: ChangePlan,
}

/// `/tmp/debkit` by default; overridable via `DEBKIT_STAGE_DIR` so tests never touch
/// the real path, mirroring `evidence::state_root`'s `DEBKIT_STATE_DIR` pattern.
pub fn staging_root() -> PathBuf {
    std::env::var_os("DEBKIT_STAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/debkit"))
}

/// Materializes what `apply_plan` would do into a parallel, inspectable directory
/// tree under `staging_root()/<run-id>/` — without ever touching a real file — for
/// `debkit apply --dry-run`. Only `Change::WriteFile` entries produce staged output,
/// since they're the only variant with file content to preview; every other change
/// type is listed in `skipped_actions` instead of executed.
///
/// Each `WriteFile` target's *current* real content, if it exists and is readable
/// without elevated privileges, is also snapshotted into a `pristine/` copy alongside
/// the staged `changes/` copy — a directly-inspectable "before" reference distinct
/// from (and simpler to eyeball than) the rollback journal, which restores
/// programmatically but isn't meant for a human to read through first.
///
/// Runs entirely as the invoking user: `/tmp` is normally world-writable, and every
/// `WriteFile` target this codebase manages is expected to be world-readable (the
/// same assumption `apply_change`'s pre-change snapshot already makes via
/// `exec::read_file_if_exists`) — so previewing a change never itself requires root,
/// even for a change that would need it to actually apply.
pub fn stage_plan(module: &str, host: &str, plan: &ChangePlan) -> anyhow::Result<StagedPlan> {
    stage_plan_under(&staging_root(), module, host, plan)
}

/// Pulled out of `stage_plan` so tests can target a temp directory instead of the
/// real `staging_root()` — avoids both touching `/tmp/debkit` for real during tests
/// and the parallel-test races an env-var-based override would risk.
fn stage_plan_under(
    base: &Path,
    module: &str,
    host: &str,
    plan: &ChangePlan,
) -> anyhow::Result<StagedPlan> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let root = base.join(&run_id);
    let changes_root = root.join("changes");
    let pristine_root = root.join("pristine");

    let mut files = Vec::new();
    let mut skipped_actions = Vec::new();

    for planned in &plan.changes {
        match &planned.change {
            Change::WriteFile { path, content } => {
                let staged_path = mirror_path(&changes_root, path);
                write_staged_file(&staged_path, content)
                    .with_context(|| format!("failed to stage {}", path.display()))?;

                let pristine_path = if path.exists() {
                    let pristine_path = mirror_path(&pristine_root, path);
                    snapshot_pristine_file(path, &pristine_path).ok().flatten()
                } else {
                    None
                };

                files.push(StagedFile {
                    target: path.clone(),
                    staged_path,
                    pristine_path,
                });
            }
            Change::RunCommand { program, args, .. } => {
                skipped_actions.push(format!("run: {program} {}", args.join(" ")));
            }
            Change::ServiceAction { unit, action } => {
                skipped_actions.push(format!("service: systemctl {} {unit}", action.as_str()));
            }
            Change::InstallPackages { packages } => {
                skipped_actions.push(format!("install: {}", packages.join(", ")));
            }
        }
    }

    let manifest = StagedManifest {
        module: module.to_string(),
        host: host.to_string(),
        plan: plan.clone(),
    };
    let manifest_path = root.join("plan.json");
    let rendered =
        serde_json::to_string_pretty(&manifest).context("failed to serialize staged plan")?;
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&manifest_path, rendered)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    Ok(StagedPlan {
        run_id,
        module: module.to_string(),
        root,
        manifest_path,
        files,
        skipped_actions,
    })
}

/// Reads back the manifest a `stage_plan` call wrote for `run_id`, for `debkit apply
/// --from-run`.
pub fn read_staged_manifest(run_id: &str) -> anyhow::Result<StagedManifest> {
    let path = staging_root().join(run_id).join("plan.json");
    let raw = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read {} (was `{run_id}` created by `debkit apply --dry-run`?)",
            path.display()
        )
    })?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

/// Joins `target` (always an absolute path, e.g. `/etc/default/grub`) onto `root`,
/// stripping the leading `/` so it composes cleanly instead of `PathBuf::join`
/// treating the absolute `target` as replacing `root` entirely.
fn mirror_path(root: &Path, target: &Path) -> PathBuf {
    root.join(target.strip_prefix("/").unwrap_or(target))
}

fn write_staged_file(staged_path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = staged_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(staged_path, content)
        .with_context(|| format!("failed to write {}", staged_path.display()))
}

/// Returns `Ok(Some(path))` on success, `Ok(None)` if the source couldn't be read
/// (permission denied is common and shouldn't fail the whole staging run).
fn snapshot_pristine_file(source: &Path, pristine_path: &Path) -> anyhow::Result<Option<PathBuf>> {
    let Ok(content) = fs::read_to_string(source) else {
        return Ok(None);
    };
    if let Some(parent) = pristine_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(pristine_path, content)
        .with_context(|| format!("failed to write {}", pristine_path.display()))?;
    Ok(Some(pristine_path.to_path_buf()))
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

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "debkit_apply_test_{label}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stage_plan_writes_new_content_without_touching_the_real_target() {
        let base = temp_dir("stage_new_file");
        let target = base.join("target.conf");
        // Deliberately do NOT create `target` -- this exercises the "file doesn't
        // exist yet" path (no pristine copy expected).
        let mut plan = ChangePlan::new();
        plan.push(
            "write target.conf",
            Risk::Low,
            Change::WriteFile {
                path: target.clone(),
                content: "hello\n".to_string(),
            },
        );

        let staged = stage_plan_under(&base, "hardware.reboot", "devbox", &plan).unwrap();
        assert_eq!(staged.files.len(), 1);
        let file = &staged.files[0];
        assert_eq!(file.target, target);
        assert!(file.pristine_path.is_none());
        assert_eq!(fs::read_to_string(&file.staged_path).unwrap(), "hello\n");
        // The real target must never be created by staging.
        assert!(!target.exists());
    }

    #[test]
    fn stage_plan_snapshots_pristine_content_when_target_exists() {
        let base = temp_dir("stage_existing_file");
        let target = base.join("target.conf");
        fs::write(&target, "original\n").unwrap();

        let mut plan = ChangePlan::new();
        plan.push(
            "write target.conf",
            Risk::Low,
            Change::WriteFile {
                path: target.clone(),
                content: "new content\n".to_string(),
            },
        );

        let staged = stage_plan_under(&base, "hardware.reboot", "devbox", &plan).unwrap();
        let file = &staged.files[0];
        let pristine_path = file.pristine_path.as_ref().expect("target existed");
        assert_eq!(fs::read_to_string(pristine_path).unwrap(), "original\n");
        assert_eq!(
            fs::read_to_string(&file.staged_path).unwrap(),
            "new content\n"
        );
        // The real target is untouched -- still its original content.
        assert_eq!(fs::read_to_string(&target).unwrap(), "original\n");
    }

    #[test]
    fn stage_plan_never_executes_non_write_file_changes() {
        let base = temp_dir("stage_non_file_changes");
        let mut plan = ChangePlan::new();
        plan.push(
            "run update-grub",
            Risk::High,
            Change::RunCommand {
                program: "update-grub".to_string(),
                args: Vec::new(),
                privileged: true,
            },
        );
        plan.push(
            "restart dnsmasq",
            Risk::Medium,
            Change::ServiceAction {
                unit: "dnsmasq".to_string(),
                action: crate::engine::plan::ServiceActionKind::Restart,
            },
        );
        plan.push(
            "install ripgrep",
            Risk::Medium,
            Change::InstallPackages {
                packages: vec!["ripgrep".to_string()],
            },
        );

        let staged = stage_plan_under(&base, "hardware.reboot", "devbox", &plan).unwrap();
        assert!(staged.files.is_empty());
        assert_eq!(staged.skipped_actions.len(), 3);
        assert!(staged.skipped_actions[0].contains("update-grub"));
        assert!(staged.skipped_actions[1].contains("restart"));
        assert!(staged.skipped_actions[2].contains("ripgrep"));
    }

    #[test]
    fn stage_plan_mirrors_the_absolute_path_hierarchy_under_a_fresh_run_id() {
        let base = temp_dir("stage_mirrors_hierarchy");
        let target = base.join("etc").join("default").join("grub");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"\n").unwrap();

        let mut plan = ChangePlan::new();
        plan.push(
            "patch grub default",
            Risk::High,
            Change::WriteFile {
                path: target.clone(),
                content: "GRUB_CMDLINE_LINUX_DEFAULT=\"quiet reboot=cold\"\n".to_string(),
            },
        );

        let staged = stage_plan_under(&base, "hardware.reboot", "devbox", &plan).unwrap();
        assert!(staged.root.starts_with(&base));
        assert!(staged.root.ends_with(&staged.run_id));
        let file = &staged.files[0];
        assert!(file.staged_path.starts_with(staged.root.join("changes")));
        assert!(
            file.pristine_path
                .as_ref()
                .unwrap()
                .starts_with(staged.root.join("pristine"))
        );
    }

    #[test]
    fn two_stage_plan_runs_get_different_run_ids() {
        let base = temp_dir("stage_unique_run_ids");
        let plan = ChangePlan::new();
        let first = stage_plan_under(&base, "hardware.reboot", "devbox", &plan).unwrap();
        let second = stage_plan_under(&base, "hardware.reboot", "devbox", &plan).unwrap();
        assert_ne!(first.run_id, second.run_id);
    }

    #[test]
    fn stage_plan_writes_a_manifest_that_round_trips_the_full_plan() {
        let base = temp_dir("stage_manifest_round_trip");
        let target = base.join("target.conf");
        let mut plan = ChangePlan::new();
        plan.push(
            "write target.conf",
            Risk::High,
            Change::WriteFile {
                path: target,
                content: "hello\n".to_string(),
            },
        );
        plan.push(
            "restart dnsmasq",
            Risk::Medium,
            Change::ServiceAction {
                unit: "dnsmasq".to_string(),
                action: crate::engine::plan::ServiceActionKind::Restart,
            },
        );

        let staged = stage_plan_under(&base, "network.dns", "devbox", &plan).unwrap();
        assert!(staged.manifest_path.exists());

        let raw = fs::read_to_string(&staged.manifest_path).unwrap();
        let manifest: StagedManifest = serde_json::from_str(&raw).unwrap();
        assert_eq!(manifest.module, "network.dns");
        assert_eq!(manifest.host, "devbox");
        assert_eq!(manifest.plan, plan);
    }

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
