use crate::config::DebkitConfig;
use crate::engine::evidence::VerificationResult;
use crate::engine::plan::ChangePlan;

/// Shared, read-only run context passed to every lifecycle method.
pub struct Context<'a> {
    pub hostname: String,
    pub config: &'a DebkitConfig,
}

/// What `discover()` found on the host for a module's resource, before any judgment about
/// whether it matches declared intent. Kept as a JSON value so `core.inspect` can
/// concatenate every module's contribution without a shared struct, and so it serializes
/// directly into `Evidence::observed`.
#[derive(Debug, Clone)]
pub struct Observation {
    /// The currently active owner of the resource, if `diagnose()` was able to resolve one
    /// unambiguously (see `engine::ownership`).
    pub owner: Option<String>,
    pub data: serde_json::Value,
    pub warnings: Vec<String>,
}

impl Observation {
    pub fn new(data: serde_json::Value) -> Self {
        Self {
            owner: None,
            data,
            warnings: Vec::new(),
        }
    }

    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

/// The result of comparing an `Observation` against declared intent. Never touches the
/// system.
#[derive(Debug, Clone, Default)]
pub struct Diagnosis {
    pub compliant: bool,
    pub findings: Vec<String>,
    /// Set when more than one owner is active for the same resource and no config
    /// override resolves it. `plan()` must refuse to proceed while this is set.
    pub conflict: Option<Vec<String>>,
}

impl Diagnosis {
    pub fn compliant() -> Self {
        Self {
            compliant: true,
            findings: Vec::new(),
            conflict: None,
        }
    }

    pub fn mismatch(findings: Vec<String>) -> Self {
        Self {
            compliant: false,
            findings,
            conflict: None,
        }
    }

    pub fn conflict(owners: Vec<String>) -> Self {
        Self {
            compliant: false,
            findings: vec![format!(
                "competing owners are active: {}",
                owners.join(", ")
            )],
            conflict: Some(owners),
        }
    }
}

/// A discoverable, diagnosable, plannable, verifiable unit of host configuration.
///
/// The four methods mirror the doc's execution lifecycle: `discover` never judges,
/// `diagnose` never touches the system, `plan` never applies, and `verify` checks
/// user-visible behavior rather than command exit status. The generic engine
/// (`engine::apply`) is responsible for turning a `ChangePlan` into applied changes,
/// checkpoints, and rollback — modules only describe intent.
pub trait Module {
    /// Dotted identifier matching the module's config section, e.g. "network.wake_on_lan".
    fn name(&self) -> &'static str;

    /// One-line human-facing summary, shown by `debkit list`.
    fn description(&self) -> &'static str;

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation>;

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis;

    /// Returns an empty `ChangePlan` when `diagnosis.compliant` is true. Must return an
    /// error (not an empty plan) if `diagnosis.conflict` is set and unresolved.
    fn plan(
        &self,
        ctx: &Context,
        observation: &Observation,
        diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan>;

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>>;
}
