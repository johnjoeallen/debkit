use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ServiceActionKind {
    EnableNow,
    Restart,
}

impl ServiceActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnableNow => "enable --now",
            Self::Restart => "restart",
        }
    }
}

/// A single system mutation a module wants to make. The engine (`engine::apply`) knows
/// how to render, apply, and reverse each variant without module-specific code.
#[derive(Debug, Clone)]
pub enum Change {
    WriteFile {
        path: PathBuf,
        content: String,
    },
    RunCommand {
        program: String,
        args: Vec<String>,
        /// Most system mutations need root (sudo). Per-user state — `git config
        /// --global`, files under the invoking user's `$HOME` — must run as that user
        /// instead; running it under sudo would silently operate on root's `$HOME`.
        privileged: bool,
    },
    ServiceAction {
        unit: String,
        action: ServiceActionKind,
    },
    /// `apt-get update` + `apt-get install -y <packages>`, run non-interactively, with
    /// each package's installed state verified afterward — the generic form of what
    /// every module that needs a package currently hand-rolls.
    InstallPackages {
        packages: Vec<String>,
    },
}

/// One planned change plus the human-facing description and risk shown in dry-run output.
#[derive(Debug, Clone)]
pub struct PlannedChange {
    pub description: String,
    pub risk: Risk,
    pub change: Change,
}

/// The ordered set of changes a module's `plan()` wants to apply. An empty plan means the
/// module is already compliant (idempotent no-op).
#[derive(Debug, Clone, Default)]
pub struct ChangePlan {
    pub changes: Vec<PlannedChange>,
}

impl ChangePlan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, description: impl Into<String>, risk: Risk, change: Change) {
        self.changes.push(PlannedChange {
            description: description.into(),
            risk,
            change,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn render_dry_run(&self) -> String {
        if self.changes.is_empty() {
            return "no changes required; already compliant".to_string();
        }
        let mut out = String::new();
        for planned in &self.changes {
            out.push_str(&format!(
                "- [{}] {}\n",
                planned.risk.as_str(),
                planned.description
            ));
            match &planned.change {
                Change::WriteFile { path, .. } => {
                    out.push_str(&format!("    write: {}\n", path.display()));
                }
                Change::RunCommand {
                    program,
                    args,
                    privileged,
                } => {
                    let prefix = if *privileged { "sudo " } else { "" };
                    out.push_str(&format!(
                        "    run: {prefix}{} {}\n",
                        program,
                        args.join(" ")
                    ));
                }
                Change::ServiceAction { unit, action } => {
                    out.push_str(&format!(
                        "    service: systemctl {} {}\n",
                        action.as_str(),
                        unit
                    ));
                }
                Change::InstallPackages { packages } => {
                    out.push_str(&format!("    install: {}\n", packages.join(" ")));
                }
            }
        }
        out
    }
}
