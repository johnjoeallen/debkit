use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::{Change, ChangePlan, Risk};

const DMI_ROOT: &str = "/sys/class/dmi/id";
const GRUB_DEFAULT_PATH: &str = "/etc/default/grub";
const GRUB_CFG_PATH: &str = "/boot/grub/grub.cfg";
const GRUB_CMDLINE_KEY: &str = "GRUB_CMDLINE_LINUX_DEFAULT";

/// Board capacities in ascending order. Used to round an observed value up to the
/// nearest "nominal" size a vendor would actually sell/market, absorbing the gap
/// between installed capacity and what `/proc/meminfo` reports (reserved for
/// firmware/iGPU/etc.) without needing a fixed percentage that would vary by platform.
const COMMON_MEMORY_CAPACITIES_GIB: &[u32] = &[
    1, 2, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048,
];

const VENDOR_SUFFIXES: &[&str] = &[
    "co., ltd",
    "co.,ltd",
    "co ltd",
    "corporation",
    "incorporated",
    "inc",
    "ltd",
];

/// One board's known-affected BIOS versions, sourced from the embedded (currently
/// empty — see module doc) or user-supplied registry at
/// `~/.config/debkit/boards/registry.yaml`.
///
/// Deliberately an explicit version list rather than a range: vendor BIOS version
/// strings (MSI's `2.AC3`, for example) aren't a consistently orderable scheme, so a
/// range comparison would either be wrong or need per-vendor parsing this module isn't
/// scoped to build. A user who hits a specific known-bad version adds it here by exact
/// string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardCompatibilityEntry {
    pub vendor: String,
    pub name: String,
    #[serde(default)]
    pub affected_bios_versions: Vec<String>,
    #[serde(default)]
    pub note: String,
    /// This board's recommended `reboot_mode` ("cold"/"warm"), used when
    /// `hardware_reboot.reboot_mode` isn't explicitly set in config — an explicit
    /// config value always wins over this. Optional: not every entry needs a
    /// recommendation (e.g. one purely documenting a known-affected BIOS version
    /// whose real fix is a flash, not a reboot-mode change).
    #[serde(default)]
    pub recommended_reboot_mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct BoardRegistryFile {
    #[serde(default)]
    boards: Vec<BoardCompatibilityEntry>,
}

/// Board/BIOS identification, scoped narrowly to detection + normalization + a
/// known-affected-BIOS lookup — not a general hardware compatibility matcher. Ported
/// from the AM5-platform troubleshooting in the requirements doc: a BIOS update can
/// change memory training such that a reboot silently drops to a lower memory speed or
/// fails to detect a DIMM, with no non-firmware signal that anything changed.
///
/// Reads straight from `/sys/class/dmi/id/*` — no `dmidecode`, no root required.
/// Missing individual fields, or no DMI support on the platform at all, is a normal
/// `None`/`dmi_available: false` outcome, not a discovery error.
///
/// The memory-capacity and known-affected-BIOS findings themselves still have no
/// automated fix — DebKit can't reseat a DIMM or safely choose to flash a BIOS on your
/// behalf. What `plan()`/`apply()` *do* manage is the actual underlying mitigation for
/// both symptoms: persisting `reboot=<mode>[,<type>]` (`reboot_mode`/`reboot_type`)
/// into `/etc/default/grub`'s `GRUB_CMDLINE_LINUX_DEFAULT` and regenerating
/// `/boot/grub/grub.cfg` via `update-grub`. `reboot_mode: cold` (the default) sets the
/// BIOS cold-boot flag, forcing a full memory retrain/POST on every future reboot —
/// both findings above stem from a *warm* reboot skipping that retrain, so this is a
/// real fix for the reboot-time symptom even though it can't touch the DIMM or BIOS
/// version directly. This is genuinely higher-risk than anything else this module
/// does (a bad `/etc/default/grub` edit can leave a system unbootable), so both
/// changes are `Risk::High` and the parsing is deliberately conservative — see
/// `patch_grub_cmdline_default`'s doc comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HardwareRebootObservation {
    dmi_available: bool,
    board_vendor: Option<String>,
    board_vendor_normalized: Option<String>,
    board_name: Option<String>,
    board_name_normalized: Option<String>,
    board_version: Option<String>,
    bios_vendor: Option<String>,
    bios_version: Option<String>,
    bios_date: Option<String>,
    registry_match: Option<BoardCompatibilityEntry>,
    current_bios_is_known_affected: bool,
    /// Observed `/proc/meminfo` `MemTotal`, rounded up to the nearest capacity in
    /// `COMMON_MEMORY_CAPACITIES_GIB`. `None` if `/proc/meminfo` couldn't be read.
    observed_memory_gib: Option<u32>,
    /// Raw content of `/etc/default/grub`, if readable.
    grub_default_current: Option<String>,
    /// Whether `GRUB_CMDLINE_LINUX_DEFAULT` already contains a `reboot=<desired>`
    /// token. `None` if the file couldn't be read, or didn't contain a recognizable
    /// `GRUB_CMDLINE_LINUX_DEFAULT="..."` line (this module never guesses at an
    /// unfamiliar layout).
    grub_default_declares_reboot_arg: Option<bool>,
    /// The full patched file content `plan()` would write, computed only when
    /// `grub_default_declares_reboot_arg == Some(false)`.
    grub_default_desired: Option<String>,
    /// Whether `/boot/grub/grub.cfg` — the generated, effective config actually read
    /// at boot — already contains the desired token. `None` if unreadable (commonly
    /// root-only permissions; `debkit diagnose` run unprivileged can't confirm this).
    grub_cfg_declares_reboot_arg: Option<bool>,
}

pub struct HardwareReboot;

impl Module for HardwareReboot {
    fn name(&self) -> &'static str {
        "hardware.reboot"
    }

    fn description(&self) -> &'static str {
        "AM5 board/BIOS identification, known-affected-BIOS registry, memory-capacity check"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let dmi = read_dmi();
        let registry = load_board_registry();
        let registry_match = dmi.find_registry_match(&registry);
        let current_bios_is_known_affected = registry_match
            .as_ref()
            .is_some_and(|entry| bios_version_is_listed(dmi.bios_version.as_deref(), entry));

        let effective_mode = effective_reboot_mode(
            &ctx.config.hardware_reboot.reboot_mode,
            registry_match.as_ref(),
        );
        let desired_arg =
            desired_reboot_arg(&effective_mode, &ctx.config.hardware_reboot.reboot_type);
        let grub_default_current = fs::read_to_string(GRUB_DEFAULT_PATH).ok();
        let grub_default_declares_reboot_arg = grub_default_current
            .as_deref()
            .and_then(grub_cmdline_default_raw_value)
            .and_then(|value| grub_cmdline_declares(&value, &desired_arg));
        let grub_default_desired = if grub_default_declares_reboot_arg == Some(false) {
            grub_default_current
                .as_deref()
                .and_then(|content| patch_grub_cmdline_default(content, &desired_arg))
        } else {
            None
        };
        let grub_cfg_declares_reboot_arg =
            read_grub_cfg().map(|content| grub_cfg_declares_reboot_arg(&content, &desired_arg));

        let data = HardwareRebootObservation {
            dmi_available: dmi.available,
            board_vendor: dmi.board_vendor,
            board_vendor_normalized: dmi.board_vendor_normalized.clone(),
            board_name: dmi.board_name,
            board_name_normalized: dmi.board_name_normalized.clone(),
            board_version: dmi.board_version,
            bios_vendor: dmi.bios_vendor,
            bios_version: dmi.bios_version,
            bios_date: dmi.bios_date,
            registry_match,
            current_bios_is_known_affected,
            observed_memory_gib: read_mem_total_kib().and_then(round_up_to_common_capacity_gib),
            grub_default_current,
            grub_default_declares_reboot_arg,
            grub_default_desired,
            grub_cfg_declares_reboot_arg,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        if !data.dmi_available {
            observation = observation.with_warning(
                "no DMI information available under /sys/class/dmi/id — board identification is unsupported on this platform"
                    .to_string(),
            );
        }
        if data.current_bios_is_known_affected {
            observation = observation.with_warning(format!(
                "BIOS `{}` is on this board's known-affected list",
                data.bios_version.as_deref().unwrap_or("unknown")
            ));
        }
        if data.grub_default_current.is_none() {
            observation = observation.with_warning(format!("could not read {GRUB_DEFAULT_PATH}"));
        } else if data.grub_default_declares_reboot_arg.is_none() {
            observation = observation.with_warning(format!(
                "{GRUB_DEFAULT_PATH} does not contain a recognizable {GRUB_CMDLINE_KEY}=\"...\" line; refusing to guess how to patch it"
            ));
        }
        if data.grub_cfg_declares_reboot_arg.is_none() {
            observation = observation.with_warning(format!(
                "could not read {GRUB_CFG_PATH} (commonly root-only permissions) — cannot confirm the effective boot config"
            ));
        }
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.hardware_reboot.enabled {
            return Diagnosis::compliant();
        }

        let data: HardwareRebootObservation = match serde_json::from_value(observation.data.clone())
        {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read hardware.reboot observation: {err}"
                )]);
            }
        };

        let mut findings = Vec::new();
        if data.current_bios_is_known_affected {
            let entry = data
                .registry_match
                .as_ref()
                .expect("current_bios_is_known_affected implies a registry match");
            let note = if entry.note.trim().is_empty() {
                "no further detail recorded".to_string()
            } else {
                entry.note.clone()
            };
            findings.push(format!(
                "BIOS `{}` on `{} {}` is a known-affected version: {note}",
                data.bios_version.as_deref().unwrap_or("unknown"),
                entry.vendor,
                entry.name,
            ));
        }

        let expected = ctx.config.hardware_reboot.expected_memory_gib;
        if expected > 0 {
            match data.observed_memory_gib {
                Some(observed) if observed != expected => {
                    findings.push(format!(
                        "observed installed memory (~{observed} GiB) does not match declared `expected_memory_gib` ({expected}) — a DIMM may have gone undetected after the last reboot/BIOS change"
                    ));
                }
                Some(_) => {}
                None => findings
                    .push("could not read /proc/meminfo to verify installed memory".to_string()),
            }
        }

        if needs_grub_mitigation(ctx.config.hardware_reboot.expected_memory_gib, &data) {
            let effective_mode = effective_reboot_mode(
                &ctx.config.hardware_reboot.reboot_mode,
                data.registry_match.as_ref(),
            );
            let desired_arg =
                desired_reboot_arg(&effective_mode, &ctx.config.hardware_reboot.reboot_type);
            match data.grub_default_declares_reboot_arg {
                Some(false) => findings.push(format!(
                    "{GRUB_DEFAULT_PATH} does not declare `reboot={desired_arg}` — a cold reboot forces full memory retraining, the actual mitigation for the findings above"
                )),
                None => findings.push(format!(
                    "could not confirm whether {GRUB_DEFAULT_PATH} declares `reboot={desired_arg}`"
                )),
                Some(true) => {}
            }
            if data.grub_cfg_declares_reboot_arg == Some(false) {
                findings.push(format!(
                    "{GRUB_CFG_PATH} does not reflect `reboot={desired_arg}` yet — run `update-grub` (or `debkit apply hardware.reboot`)"
                ));
            }
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
        if !ctx.config.hardware_reboot.enabled || diagnosis.compliant {
            return Ok(plan);
        }

        // See module doc comment: the memory-capacity and known-affected-BIOS
        // findings themselves have no automated fix. This only ever acts on the
        // shared grub-level mitigation for both -- and only when one of them is
        // confirmed, not just because reboot_mode/reboot_type are declared. A
        // passing memory check and no BIOS-registry match means grub is left alone
        // entirely, even if it doesn't already say reboot=cold.
        let data: HardwareRebootObservation = serde_json::from_value(observation.data.clone())?;
        if !needs_grub_mitigation(ctx.config.hardware_reboot.expected_memory_gib, &data) {
            return Ok(plan);
        }
        let effective_mode = effective_reboot_mode(
            &ctx.config.hardware_reboot.reboot_mode,
            data.registry_match.as_ref(),
        );
        let desired_arg =
            desired_reboot_arg(&effective_mode, &ctx.config.hardware_reboot.reboot_type);

        let needs_default_write = data.grub_default_declares_reboot_arg == Some(false)
            && data.grub_default_desired.is_some();
        let effective_stale = data.grub_cfg_declares_reboot_arg == Some(false);
        if !needs_default_write && !effective_stale {
            return Ok(plan);
        }

        let Some(update_grub) =
            resolve_command(&["update-grub", "/usr/sbin/update-grub", "/sbin/update-grub"])
        else {
            // No update-grub on this host -- not a GRUB-managed boot setup DebKit
            // can act on. Writing /etc/default/grub with nothing to regenerate it
            // would just leave a half-applied, misleading state.
            return Ok(plan);
        };

        if needs_default_write {
            plan.push(
                format!("declare `reboot={desired_arg}` in {GRUB_DEFAULT_PATH}"),
                Risk::High,
                Change::WriteFile {
                    path: PathBuf::from(GRUB_DEFAULT_PATH),
                    content: data
                        .grub_default_desired
                        .clone()
                        .expect("needs_default_write implies grub_default_desired is Some"),
                },
            );
        }
        plan.push(
            "regenerate /boot/grub/grub.cfg (update-grub)",
            Risk::High,
            Change::RunCommand {
                program: update_grub.to_string(),
                args: Vec::new(),
                privileged: true,
            },
        );

        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        if !ctx.config.hardware_reboot.enabled {
            return Ok(vec![VerificationResult::skipped(
                "hardware.reboot",
                "disabled in config",
            )]);
        }

        let mut checks = Vec::new();

        let expected = ctx.config.hardware_reboot.expected_memory_gib;
        if expected == 0 {
            checks.push(VerificationResult::skipped(
                "installed memory matches expected capacity",
                "`expected_memory_gib` not declared",
            ));
        } else {
            match read_mem_total_kib().and_then(round_up_to_common_capacity_gib) {
                Some(observed) if observed == expected => {
                    checks.push(VerificationResult::pass(
                        "installed memory matches expected capacity",
                    ));
                }
                Some(observed) => checks.push(VerificationResult::fail(
                    "installed memory matches expected capacity",
                    format!(
                        "observed ~{observed} GiB, expected {expected} GiB — check whether all DIMMs are detected (e.g. `sudo dmidecode --type 17`)"
                    ),
                )),
                None => checks.push(VerificationResult::fail(
                    "installed memory matches expected capacity",
                    "could not read /proc/meminfo",
                )),
            }
        }

        let dmi = read_dmi();
        let registry = load_board_registry();
        let registry_match = dmi.find_registry_match(&registry);
        match &registry_match {
            Some(entry) if bios_version_is_listed(dmi.bios_version.as_deref(), entry) => {
                checks.push(VerificationResult::fail(
                    "BIOS is not on this board's known-affected list",
                    format!(
                        "`{}` is known-affected: {}",
                        dmi.bios_version.as_deref().unwrap_or("unknown"),
                        entry.note
                    ),
                ));
            }
            Some(_) => checks.push(VerificationResult::pass(
                "BIOS is not on this board's known-affected list",
            )),
            None => checks.push(VerificationResult::skipped(
                "BIOS is not on this board's known-affected list",
                "no registry entry for this board",
            )),
        }

        let effective_mode = effective_reboot_mode(
            &ctx.config.hardware_reboot.reboot_mode,
            registry_match.as_ref(),
        );
        let desired_arg =
            desired_reboot_arg(&effective_mode, &ctx.config.hardware_reboot.reboot_type);
        let check_name = format!("{GRUB_CFG_PATH} declares reboot={desired_arg}");
        match read_grub_cfg().map(|content| grub_cfg_declares_reboot_arg(&content, &desired_arg)) {
            Some(true) => checks.push(VerificationResult::pass(check_name)),
            Some(false) => checks.push(VerificationResult::fail(
                check_name,
                "not present in the generated config — run `update-grub` or `debkit apply hardware.reboot`",
            )),
            None => checks.push(VerificationResult::skipped(
                check_name,
                "could not read /boot/grub/grub.cfg (commonly root-only permissions)",
            )),
        }

        Ok(checks)
    }
}

struct DmiInfo {
    available: bool,
    board_vendor: Option<String>,
    board_vendor_normalized: Option<String>,
    board_name: Option<String>,
    board_name_normalized: Option<String>,
    board_version: Option<String>,
    bios_vendor: Option<String>,
    bios_version: Option<String>,
    bios_date: Option<String>,
}

impl DmiInfo {
    fn find_registry_match(
        &self,
        registry: &[BoardCompatibilityEntry],
    ) -> Option<BoardCompatibilityEntry> {
        let vendor = self.board_vendor_normalized.as_deref()?;
        let name = self.board_name_normalized.as_deref()?;
        registry
            .iter()
            .find(|entry| {
                normalize_board_field(&entry.vendor) == vendor
                    && normalize_board_field(&entry.name) == name
            })
            .cloned()
    }
}

fn read_dmi_field(name: &str) -> Option<String> {
    let raw = fs::read_to_string(Path::new(DMI_ROOT).join(name)).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn read_dmi() -> DmiInfo {
    let board_vendor = read_dmi_field("board_vendor");
    let board_name = read_dmi_field("board_name");
    DmiInfo {
        available: Path::new(DMI_ROOT).exists(),
        board_vendor_normalized: board_vendor.as_deref().map(normalize_board_field),
        board_name_normalized: board_name.as_deref().map(normalize_board_field),
        board_vendor,
        board_name,
        board_version: read_dmi_field("board_version"),
        bios_vendor: read_dmi_field("bios_vendor"),
        bios_version: read_dmi_field("bios_version"),
        bios_date: read_dmi_field("bios_date"),
    }
}

/// Case-folds, collapses whitespace, and strips common vendor-suffix noise (`Co.,
/// Ltd.`, `Corporation`, ...) so registry entries can be authored without needing to
/// match a vendor's exact legal-entity string.
fn normalize_board_field(raw: &str) -> String {
    let mut current = raw.trim().to_lowercase();
    loop {
        let trimmed = current.trim_end_matches([',', '.', ' ']).to_string();
        let stripped = VENDOR_SUFFIXES
            .iter()
            .find_map(|suffix| trimmed.strip_suffix(suffix).map(str::to_string));
        match stripped {
            Some(next) => current = next,
            None => {
                current = trimmed;
                break;
            }
        }
    }
    current.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn bios_version_is_listed(bios_version: Option<&str>, entry: &BoardCompatibilityEntry) -> bool {
    let Some(bios_version) = bios_version else {
        return false;
    };
    let current = bios_version.trim().to_lowercase();
    entry
        .affected_bios_versions
        .iter()
        .any(|listed| listed.trim().to_lowercase() == current)
}

fn read_mem_total_kib() -> Option<u64> {
    let raw = fs::read_to_string("/proc/meminfo").ok()?;
    raw.lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
}

/// Rounds an observed `MemTotal` up to the smallest common capacity whose nominal size
/// (allowing 2% headroom, since the OS-visible total is never larger than what's
/// installed) could plausibly explain it. This is a coarse capacity check, not a speed
/// check — it catches a DIMM going undetected, not memory running at a lower MT/s than
/// configured (which needs `dmidecode --type 17` and root, out of scope here).
fn round_up_to_common_capacity_gib(total_kib: u64) -> Option<u32> {
    let total_gib = total_kib as f64 / (1024.0 * 1024.0);
    COMMON_MEMORY_CAPACITIES_GIB
        .iter()
        .copied()
        .find(|&capacity| total_gib <= capacity as f64 * 1.02)
}

fn desired_reboot_arg(mode: &str, kind: &str) -> String {
    if kind.is_empty() {
        mode.to_string()
    } else {
        format!("{mode},{kind}")
    }
}

/// Resolves the actual `reboot_mode` to use: an explicit, non-empty
/// `hardware_reboot.reboot_mode` in config always wins; otherwise falls back to the
/// matched board's `recommended_reboot_mode` from the registry, if any; otherwise
/// falls back to `DEFAULT_REBOOT_MODE` ("cold"). This is what lets the registry
/// "just know" the right value for a recognized board without requiring the user to
/// declare `reboot_mode` themselves, while still letting them override it explicitly
/// if they want something different.
fn effective_reboot_mode(
    config_mode: &str,
    registry_match: Option<&BoardCompatibilityEntry>,
) -> String {
    if !config_mode.is_empty() {
        return config_mode.to_string();
    }
    if let Some(recommended) =
        registry_match.and_then(|entry| entry.recommended_reboot_mode.as_deref())
        && !recommended.is_empty()
    {
        return recommended.to_string();
    }
    crate::config::DEFAULT_REBOOT_MODE.to_string()
}

/// Whether there's *confirmed* evidence the grub mitigation should apply: a
/// memory-capacity mismatch, the current BIOS actually matching a known-affected
/// registry entry, or the board itself matching a registry entry that carries a
/// `recommended_reboot_mode` — a recognized board the registry already has a known
/// answer for wins outright, without needing its own separate confirmed-bad
/// BIOS/memory finding first. Deliberately does NOT trigger on "could not determine"
/// (e.g. `/proc/meminfo` unreadable): absence of information isn't evidence of a
/// problem, so it shouldn't justify touching the bootloader config. Declaring
/// `reboot_mode`/`reboot_type` and enabling the module isn't, by itself, sufficient
/// reason to modify grub — there has to be an actual signal first, whether that's a
/// live mismatch or a registry entry that already knows this board.
fn needs_grub_mitigation(expected_memory_gib: u32, data: &HardwareRebootObservation) -> bool {
    if data.current_bios_is_known_affected {
        return true;
    }
    if expected_memory_gib > 0
        && let Some(observed) = data.observed_memory_gib
        && observed != expected_memory_gib
    {
        return true;
    }
    if data
        .registry_match
        .as_ref()
        .is_some_and(|entry| entry.recommended_reboot_mode.is_some())
    {
        return true;
    }
    false
}

/// Extracts the raw `"..."`/`'...'` value (including quotes) from
/// `/etc/default/grub`'s active (non-commented) `GRUB_CMDLINE_LINUX_DEFAULT=` line.
/// Exact-matches the key up to `=` so it's never confused with the separate
/// `GRUB_CMDLINE_LINUX` variable (no `_DEFAULT` suffix) — real Debian configs
/// commonly have both, and this host's own `/etc/default/grub` has exactly that
/// shape (a commented `GRUB_CMDLINE_LINUX=` template line, plus an active one
/// further down). `None` if no matching line is found.
fn grub_cmdline_default_raw_value(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            return None;
        }
        trimmed
            .strip_prefix(GRUB_CMDLINE_KEY)?
            .strip_prefix('=')
            .map(str::to_string)
    })
}

/// Whether the quoted value already contains a `reboot=<desired_arg>` token
/// (anywhere, not just at the end — see `patch_quoted_value`'s doc comment for why
/// position matters). `None` if `value` isn't a simple `"..."`/`'...'` quoted string.
fn grub_cmdline_declares(value: &str, desired_arg: &str) -> Option<bool> {
    let inner = unquote(value)?;
    let desired_token = format!("reboot={desired_arg}");
    Some(inner.split_whitespace().any(|token| token == desired_token))
}

fn unquote(value: &str) -> Option<&str> {
    let value = value.trim_end();
    let quote = value.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    value.strip_prefix(quote)?.strip_suffix(quote)
}

/// Replaces any existing `reboot=...` token in `value`'s quoted contents with
/// `reboot=<desired_arg>` (appending it if none was present) and re-quotes with the
/// same quote character. Deliberately does NOT try to preserve the original token's
/// position — it filters out any existing `reboot=` token and appends the new one at
/// the end. That's fine for `grub_cmdline_declares`'s "is it there at all" check
/// (position-independent), but means `patch_grub_cmdline_default`'s output can differ
/// byte-for-byte from an already-compliant file if the existing token wasn't already
/// last; callers must use the semantic `grub_cmdline_declares` check to decide
/// whether a write is needed at all, never raw string equality.
fn patch_quoted_value(value: &str, desired_arg: &str) -> Option<String> {
    let quote = value.trim_end().chars().next()?;
    let inner = unquote(value)?;
    let mut tokens: Vec<String> = inner
        .split_whitespace()
        .filter(|token| !token.starts_with("reboot="))
        .map(str::to_string)
        .collect();
    tokens.push(format!("reboot={desired_arg}"));
    Some(format!("{quote}{}{quote}", tokens.join(" ")))
}

/// Patches `/etc/default/grub`'s full content, replacing only the
/// `GRUB_CMDLINE_LINUX_DEFAULT=` line's value and leaving every other line — including
/// a separate active `GRUB_CMDLINE_LINUX=` line, comments, blank lines — untouched.
/// Returns `None` if no matching line was found (this module never guesses at an
/// unfamiliar grub layout) or the line's value wasn't a simple quoted string.
fn patch_grub_cmdline_default(current: &str, desired_arg: &str) -> Option<String> {
    let mut found = false;
    let mut patched_lines: Vec<String> = Vec::new();
    for line in current.lines() {
        let trimmed = line.trim_start();
        if !found
            && !trimmed.starts_with('#')
            && let Some(rest) = trimmed.strip_prefix(GRUB_CMDLINE_KEY)
            && let Some(value) = rest.strip_prefix('=')
            && let Some(patched_value) = patch_quoted_value(value, desired_arg)
        {
            found = true;
            patched_lines.push(format!("{GRUB_CMDLINE_KEY}={patched_value}"));
            continue;
        }
        patched_lines.push(line.to_string());
    }
    if !found {
        return None;
    }
    let mut result = patched_lines.join("\n");
    if current.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

fn read_grub_cfg() -> Option<String> {
    fs::read_to_string(GRUB_CFG_PATH).ok()
}

/// Whether the desired `reboot=<arg>` token appears anywhere in the generated
/// `grub.cfg` (across any boot entry) — a coarse but real functional signal, not a
/// per-entry guarantee.
fn grub_cfg_declares_reboot_arg(grub_cfg: &str, desired_arg: &str) -> bool {
    let desired_token = format!("reboot={desired_arg}");
    grub_cfg
        .lines()
        .any(|line| line.split_whitespace().any(|token| token == desired_token))
}

/// `update-grub` lives in `/usr/sbin`, which typically isn't on a regular user's
/// `PATH` on Debian — same footgun `network.firewall` hit with `iptables`/`nft`.
/// Tries the bare name first (respects `PATH` if it does include sbin), then the
/// well-known absolute paths.
fn resolve_command(candidates: &[&'static str]) -> Option<&'static str> {
    candidates.iter().copied().find(|candidate| {
        if candidate.starts_with('/') {
            Path::new(candidate).exists()
        } else {
            exec::command_available(candidate)
        }
    })
}

fn board_registry_path(home: &Path) -> PathBuf {
    home.join(".config")
        .join("debkit")
        .join("boards")
        .join("registry.yaml")
}

/// Installed by the .deb package (see `Cargo.toml`'s `[package.metadata.deb]` assets
/// and `data/boards/registry.yaml` in the repo) — ships empty deliberately, same
/// reasoning as `embedded_board_registry`. A real entry a user verifies belongs in a
/// PR to that file so every user with the same board benefits; a local-only entry
/// belongs in `~/.config/debkit/boards/registry.yaml` instead, which is merged on top
/// of this one and always wins on a conflict.
const SYSTEM_BOARD_REGISTRY_PATH: &str = "/usr/share/debkit/boards/registry.yaml";

/// Currently empty: this plan deliberately doesn't ship fabricated known-affected-BIOS
/// data (vendor compatibility notes are the kind of thing that's easy to get wrong and
/// costly to trust incorrectly). The mechanism is real and ready — see
/// `SYSTEM_BOARD_REGISTRY_PATH` (packaged defaults) and `board_registry_path`
/// (per-user additions).
fn embedded_board_registry() -> Vec<BoardCompatibilityEntry> {
    Vec::new()
}

/// Merges `path`'s entries on top of `base`: same vendor+name (normalized) replaces
/// the existing entry rather than duplicating it. A missing or unparsable file just
/// returns `base` unchanged — a package that hasn't installed the system registry yet,
/// or a user who's never created their own override file, are both normal states.
fn merge_board_registry(
    base: Vec<BoardCompatibilityEntry>,
    path: &Path,
) -> Vec<BoardCompatibilityEntry> {
    let mut registry = base;
    let Ok(raw) = fs::read_to_string(path) else {
        return registry;
    };
    let Ok(file) = serde_yaml_ng::from_str::<BoardRegistryFile>(&raw) else {
        return registry;
    };
    for entry in file.boards {
        let key = (
            normalize_board_field(&entry.vendor),
            normalize_board_field(&entry.name),
        );
        registry.retain(|existing| {
            (
                normalize_board_field(&existing.vendor),
                normalize_board_field(&existing.name),
            ) != key
        });
        registry.push(entry);
    }
    registry
}

/// Three tiers, each overriding the last on a vendor+name conflict: the compiled-in
/// (empty) default, the .deb-packaged system registry, then the user's own
/// `~/.config/debkit/boards/registry.yaml`.
fn load_board_registry() -> Vec<BoardCompatibilityEntry> {
    let registry = merge_board_registry(
        embedded_board_registry(),
        Path::new(SYSTEM_BOARD_REGISTRY_PATH),
    );
    match crate::config::home_dir() {
        Ok(home) => merge_board_registry(registry, &board_registry_path(&home)),
        Err(_) => registry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(HardwareReboot.name(), "hardware.reboot");
    }

    #[test]
    fn normalizes_vendor_suffix_and_case() {
        assert_eq!(
            normalize_board_field("Micro-Star International Co., Ltd."),
            "micro-star international"
        );
        assert_eq!(
            normalize_board_field("ASUSTeK COMPUTER INC."),
            "asustek computer"
        );
    }

    #[test]
    fn normalizes_board_name_without_over_stripping() {
        assert_eq!(
            normalize_board_field("MAG X870E TOMAHAWK WIFI (MS-7E59)"),
            "mag x870e tomahawk wifi (ms-7e59)"
        );
    }

    #[test]
    fn collapses_internal_whitespace() {
        assert_eq!(normalize_board_field("  Foo   Bar  "), "foo bar");
    }

    #[test]
    fn rounds_up_to_nearest_common_capacity() {
        // 128 GiB nominal, ~123.2 GiB observed (reserved memory) - matches this
        // machine's real /proc/meminfo reading.
        assert_eq!(round_up_to_common_capacity_gib(129_229_856), Some(128));
        // Half of 128 GiB undetected -> should NOT round back up to 128.
        assert_eq!(round_up_to_common_capacity_gib(64_000_000), Some(64));
        assert_eq!(round_up_to_common_capacity_gib(8_200_000), Some(8));
    }

    #[test]
    fn bios_version_listed_is_case_and_whitespace_insensitive() {
        let entry = BoardCompatibilityEntry {
            vendor: "Micro-Star International".to_string(),
            name: "MAG X870E TOMAHAWK WIFI (MS-7E59)".to_string(),
            affected_bios_versions: vec![" 2.AC3 ".to_string()],
            note: "known regression".to_string(),
            recommended_reboot_mode: None,
        };
        assert!(bios_version_is_listed(Some("2.ac3"), &entry));
        assert!(!bios_version_is_listed(Some("2.AC4"), &entry));
        assert!(!bios_version_is_listed(None, &entry));
    }

    #[test]
    fn registry_from_path_merges_and_overrides_embedded() {
        let dir = std::env::temp_dir().join(format!(
            "debkit_hardware_reboot_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("registry.yaml");
        fs::write(
            &path,
            "boards:\n  - vendor: Micro-Star International\n    name: MAG X870E TOMAHAWK WIFI (MS-7E59)\n    affected_bios_versions: [\"2.AC3\"]\n    note: test entry\n",
        )
        .unwrap();

        let registry = merge_board_registry(embedded_board_registry(), &path);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry[0].vendor, "Micro-Star International");
        assert_eq!(registry[0].affected_bios_versions, vec!["2.AC3"]);
    }

    #[test]
    fn registry_from_missing_path_falls_back_to_embedded() {
        let registry = merge_board_registry(
            embedded_board_registry(),
            Path::new("/nonexistent/registry.yaml"),
        );
        assert_eq!(registry, embedded_board_registry());
    }

    /// The packaged system registry ships defaults; a user's own file overrides them
    /// on a matching vendor+name, exactly like `load_board_registry`'s three-tier
    /// chain (embedded -> system -> user) is meant to behave.
    #[test]
    fn merge_board_registry_lets_a_later_tier_override_an_earlier_one() {
        let dir = std::env::temp_dir().join(format!(
            "debkit_hardware_reboot_tier_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();

        let system_path = dir.join("system.yaml");
        fs::write(
            &system_path,
            "boards:\n  - vendor: Acme\n    name: Board One\n    affected_bios_versions: [\"1.0\"]\n    note: system default\n",
        )
        .unwrap();
        let user_path = dir.join("user.yaml");
        fs::write(
            &user_path,
            "boards:\n  - vendor: Acme\n    name: Board One\n    affected_bios_versions: [\"1.0\", \"1.1\"]\n    note: user override\n",
        )
        .unwrap();

        let registry = merge_board_registry(Vec::new(), &system_path);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry[0].note, "system default");

        let registry = merge_board_registry(registry, &user_path);
        assert_eq!(
            registry.len(),
            1,
            "same vendor+name replaces, not duplicates"
        );
        assert_eq!(registry[0].note, "user override");
        assert_eq!(
            registry[0].affected_bios_versions,
            vec!["1.0".to_string(), "1.1".to_string()]
        );
    }

    /// Regression guard: the file actually shipped in the .deb
    /// (data/boards/registry.yaml in the repo, installed to
    /// SYSTEM_BOARD_REGISTRY_PATH) must always parse as valid `BoardRegistryFile`
    /// YAML, even though it deliberately has zero entries.
    #[test]
    fn packaged_registry_file_parses_as_valid_yaml() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/boards/registry.yaml");
        let raw = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let file: BoardRegistryFile =
            serde_yaml_ng::from_str(&raw).expect("packaged registry.yaml must be valid YAML");
        for entry in &file.boards {
            assert!(!entry.vendor.is_empty(), "entry has an empty vendor");
            assert!(!entry.name.is_empty(), "entry has an empty name");
        }
    }

    fn observation(data: HardwareRebootObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn base_data() -> HardwareRebootObservation {
        HardwareRebootObservation {
            dmi_available: true,
            board_vendor: Some("Micro-Star International Co., Ltd.".to_string()),
            board_vendor_normalized: Some("micro-star international".to_string()),
            board_name: Some("MAG X870E TOMAHAWK WIFI (MS-7E59)".to_string()),
            board_name_normalized: Some("mag x870e tomahawk wifi (ms-7e59)".to_string()),
            board_version: Some("2.0".to_string()),
            bios_vendor: Some("American Megatrends International, LLC.".to_string()),
            bios_version: Some("2.AC3".to_string()),
            bios_date: Some("06/25/2026".to_string()),
            registry_match: None,
            current_bios_is_known_affected: false,
            observed_memory_gib: Some(128),
            grub_default_current: Some(
                "GRUB_CMDLINE_LINUX_DEFAULT=\"quiet reboot=cold\"\n".to_string(),
            ),
            grub_default_declares_reboot_arg: Some(true),
            grub_default_desired: None,
            grub_cfg_declares_reboot_arg: Some(true),
        }
    }

    fn config(enabled: bool) -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.hardware_reboot.enabled = enabled;
        config
    }

    #[test]
    fn compliant_when_nothing_flagged() {
        let config = config(true);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
        let plan = HardwareReboot
            .plan(&ctx, &observation(base_data()), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn mismatch_when_bios_is_known_affected() {
        let mut data = base_data();
        data.current_bios_is_known_affected = true;
        data.registry_match = Some(BoardCompatibilityEntry {
            vendor: "Micro-Star International".to_string(),
            name: "MAG X870E TOMAHAWK WIFI (MS-7E59)".to_string(),
            affected_bios_versions: vec!["2.AC3".to_string()],
            note: "reverts EXPO profile after flash".to_string(),
            recommended_reboot_mode: None,
        });
        let config = config(true);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        assert!(diagnosis.findings[0].contains("known-affected version"));
        let plan = HardwareReboot
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty(), "no automated fix exists for this finding");
    }

    #[test]
    fn mismatch_when_memory_capacity_drifts_from_expected() {
        let mut data = base_data();
        data.observed_memory_gib = Some(64);
        let mut config = config(true);
        config.hardware_reboot.expected_memory_gib = 128;
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data));
        assert!(!diagnosis.compliant);
        assert!(diagnosis.findings[0].contains("does not match declared"));
    }

    #[test]
    fn compliant_when_memory_matches_expected() {
        let mut config = config(true);
        config.hardware_reboot.expected_memory_gib = 128;
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
    }

    #[test]
    fn expected_memory_zero_skips_the_check() {
        let mut data = base_data();
        data.observed_memory_gib = Some(64);
        let config = config(true);
        assert_eq!(config.hardware_reboot.expected_memory_gib, 0);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    #[test]
    fn disabled_config_is_always_compliant() {
        let mut data = base_data();
        data.current_bios_is_known_affected = true;
        let config = config(false);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    #[test]
    fn desired_reboot_arg_omits_type_when_empty() {
        assert_eq!(desired_reboot_arg("cold", ""), "cold");
        assert_eq!(desired_reboot_arg("cold", "acpi"), "cold,acpi");
    }

    /// This is this host's real /etc/default/grub content (captured live while
    /// building this module) -- it has exactly the tricky shape the parser needs to
    /// get right: a commented GRUB_CMDLINE_LINUX= template line near the top, and a
    /// separate *active* GRUB_CMDLINE_LINUX= line (no _DEFAULT) near the bottom, which
    /// must never be confused with GRUB_CMDLINE_LINUX_DEFAULT.
    const REAL_GRUB_DEFAULT_SAMPLE: &str = "\
# If you change this file or any /etc/default/grub.d/*.cfg file,
# run 'update-grub' afterwards to update /boot/grub/grub.cfg.

GRUB_DEFAULT=0
GRUB_TIMEOUT=5
GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"
#GRUB_CMDLINE_LINUX=\"acpi=force\"

GRUB_DISABLE_OS_PROBER=false
GRUB_CMDLINE_LINUX=\"acpi=force\"
";

    #[test]
    fn finds_active_default_line_ignoring_commented_and_non_default_lines() {
        assert_eq!(
            grub_cmdline_default_raw_value(REAL_GRUB_DEFAULT_SAMPLE),
            Some("\"quiet\"".to_string())
        );
    }

    #[test]
    fn declares_checks_are_position_independent_and_exact_token_match() {
        assert_eq!(grub_cmdline_declares("\"reboot=cold\"", "cold"), Some(true));
        assert_eq!(
            grub_cmdline_declares("\"quiet reboot=cold splash\"", "cold"),
            Some(true)
        );
        assert_eq!(
            grub_cmdline_declares("\"reboot=warm\"", "cold"),
            Some(false)
        );
        // Not a substring match -- "reboot=cold,acpi" must not satisfy "cold".
        assert_eq!(
            grub_cmdline_declares("\"reboot=cold,acpi\"", "cold"),
            Some(false)
        );
        assert_eq!(grub_cmdline_declares("unquoted", "cold"), None);
    }

    #[test]
    fn patch_quoted_value_replaces_existing_reboot_token() {
        assert_eq!(
            patch_quoted_value("\"quiet reboot=warm splash\"", "cold"),
            Some("\"quiet splash reboot=cold\"".to_string())
        );
    }

    #[test]
    fn patch_quoted_value_appends_when_absent() {
        assert_eq!(
            patch_quoted_value("\"quiet\"", "cold"),
            Some("\"quiet reboot=cold\"".to_string())
        );
    }

    #[test]
    fn patch_grub_cmdline_default_only_touches_the_default_line() {
        let patched = patch_grub_cmdline_default(REAL_GRUB_DEFAULT_SAMPLE, "cold,acpi").unwrap();
        assert!(patched.contains("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet reboot=cold,acpi\""));
        // The unrelated active GRUB_CMDLINE_LINUX= line (no _DEFAULT) must survive
        // untouched -- this is the exact real-world case that would break a naive
        // prefix match.
        assert!(patched.contains("GRUB_CMDLINE_LINUX=\"acpi=force\"\n"));
        assert!(patched.contains("#GRUB_CMDLINE_LINUX=\"acpi=force\"\n"));
        assert!(patched.ends_with('\n'));
    }

    #[test]
    fn patch_grub_cmdline_default_none_when_no_matching_line() {
        assert_eq!(patch_grub_cmdline_default("GRUB_TIMEOUT=5\n", "cold"), None);
    }

    #[test]
    fn grub_cfg_reboot_arg_check_is_a_substring_of_lines_not_whole_content() {
        let cfg = "linux /boot/vmlinuz root=UUID=x ro quiet reboot=cold\ninitrd /boot/initrd.img\n";
        assert!(grub_cfg_declares_reboot_arg(cfg, "cold"));
        assert!(!grub_cfg_declares_reboot_arg(cfg, "warm"));
    }

    fn registry_entry_with_recommendation(mode: &str) -> BoardCompatibilityEntry {
        BoardCompatibilityEntry {
            vendor: "Micro-Star International".to_string(),
            name: "MAG X870E TOMAHAWK WIFI (MS-7E59)".to_string(),
            affected_bios_versions: Vec::new(),
            note: String::new(),
            recommended_reboot_mode: Some(mode.to_string()),
        }
    }

    #[test]
    fn effective_reboot_mode_uses_explicit_config_value_over_registry() {
        let entry = registry_entry_with_recommendation("warm");
        assert_eq!(effective_reboot_mode("cold", Some(&entry)), "cold");
    }

    #[test]
    fn effective_reboot_mode_falls_back_to_registry_recommendation_when_config_empty() {
        let entry = registry_entry_with_recommendation("cold");
        assert_eq!(effective_reboot_mode("", Some(&entry)), "cold");
    }

    #[test]
    fn effective_reboot_mode_falls_back_to_default_when_config_and_registry_are_both_empty() {
        assert_eq!(
            effective_reboot_mode("", None),
            crate::config::DEFAULT_REBOOT_MODE
        );

        let entry = BoardCompatibilityEntry {
            vendor: "Micro-Star International".to_string(),
            name: "MAG X870E TOMAHAWK WIFI (MS-7E59)".to_string(),
            affected_bios_versions: Vec::new(),
            note: "known regression".to_string(),
            recommended_reboot_mode: None,
        };
        assert_eq!(
            effective_reboot_mode("", Some(&entry)),
            crate::config::DEFAULT_REBOOT_MODE
        );
    }

    fn config_with_reboot_args(mode: &str, kind: &str) -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.hardware_reboot.enabled = true;
        config.hardware_reboot.reboot_mode = mode.to_string();
        config.hardware_reboot.reboot_type = kind.to_string();
        config
    }

    fn config_with_memory_mismatch(
        mode: &str,
        kind: &str,
        expected_memory_gib: u32,
    ) -> crate::config::DebkitConfig {
        let mut config = config_with_reboot_args(mode, kind);
        config.hardware_reboot.expected_memory_gib = expected_memory_gib;
        config
    }

    #[test]
    fn compliant_when_grub_already_declares_desired_arg() {
        let config = config_with_reboot_args("cold", "");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
    }

    /// The exact real-world scenario that motivated `needs_grub_mitigation`: memory
    /// capacity checks out fine and there's no BIOS-registry match, so grub is left
    /// alone entirely -- even though it doesn't declare reboot=cold. Enabling the
    /// module and declaring reboot_mode is not, by itself, a reason to touch grub.
    #[test]
    fn grub_not_declaring_the_arg_is_compliant_when_memory_and_bios_are_both_fine() {
        let mut data = base_data();
        data.grub_default_current = Some("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"\n".to_string());
        data.grub_default_declares_reboot_arg = Some(false);
        data.grub_default_desired =
            patch_grub_cmdline_default(data.grub_default_current.as_ref().unwrap(), "cold");
        data.grub_cfg_declares_reboot_arg = Some(false);
        data.observed_memory_gib = Some(128); // matches expected_memory_gib below
        data.current_bios_is_known_affected = false;

        let config = config_with_memory_mismatch("cold", "", 128);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = HardwareReboot
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(
            plan.is_empty(),
            "no confirmed problem -- grub must not be touched"
        );
    }

    /// Same as above but `expected_memory_gib` isn't declared at all (0, the default)
    /// -- the memory check is skipped entirely, so there's even less basis to act.
    #[test]
    fn grub_not_declaring_the_arg_is_compliant_when_memory_check_is_undeclared() {
        let mut data = base_data();
        data.grub_default_declares_reboot_arg = Some(false);
        data.grub_cfg_declares_reboot_arg = Some(false);
        let config = config_with_reboot_args("cold", ""); // expected_memory_gib: 0
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    /// A registry entry matching this exact board, carrying a
    /// `recommended_reboot_mode`, is itself enough to justify the mitigation --
    /// even with no confirmed memory mismatch and no known-affected BIOS version.
    /// The registry already has a known answer for this board, so it wins outright.
    #[test]
    fn mismatch_when_registry_match_carries_a_recommended_reboot_mode() {
        let mut data = base_data();
        data.grub_default_current = Some("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"\n".to_string());
        data.grub_default_declares_reboot_arg = Some(false);
        data.grub_default_desired =
            patch_grub_cmdline_default(data.grub_default_current.as_ref().unwrap(), "cold");
        data.grub_cfg_declares_reboot_arg = Some(false);
        data.observed_memory_gib = Some(128); // matches expected_memory_gib below
        data.current_bios_is_known_affected = false;
        data.registry_match = Some(registry_entry_with_recommendation("cold"));

        let config = config_with_memory_mismatch("", "", 128);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = HardwareReboot
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(
            !plan.is_empty(),
            "a recognized board with a registry recommendation should get the mitigation"
        );
    }

    /// A registry match with no `recommended_reboot_mode` (e.g. an entry that only
    /// documents a known-affected BIOS version the current BIOS doesn't match) does
    /// NOT by itself justify the mitigation -- there's nothing actionable in it.
    #[test]
    fn compliant_when_registry_matches_but_has_no_recommendation_and_nothing_else_is_confirmed() {
        let mut data = base_data();
        data.grub_default_declares_reboot_arg = Some(false);
        data.grub_cfg_declares_reboot_arg = Some(false);
        data.observed_memory_gib = Some(128);
        data.current_bios_is_known_affected = false;
        data.registry_match = Some(BoardCompatibilityEntry {
            vendor: "Micro-Star International".to_string(),
            name: "MAG X870E TOMAHAWK WIFI (MS-7E59)".to_string(),
            affected_bios_versions: vec!["9.99".to_string()],
            note: "unrelated regression on a different BIOS version".to_string(),
            recommended_reboot_mode: None,
        });

        let config = config_with_memory_mismatch("", "", 128);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    #[test]
    fn mismatch_and_plan_when_grub_default_missing_the_arg_and_memory_mismatched() {
        let mut data = base_data();
        data.grub_default_current = Some("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"\n".to_string());
        data.grub_default_declares_reboot_arg = Some(false);
        data.grub_default_desired =
            patch_grub_cmdline_default(data.grub_default_current.as_ref().unwrap(), "cold");
        data.grub_cfg_declares_reboot_arg = Some(false);
        data.observed_memory_gib = Some(64); // a DIMM went undetected

        let config = config_with_memory_mismatch("cold", "", 128);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|f| f.contains("does not declare `reboot=cold`"))
        );

        // Can't assert on the actual plan() content here without update-grub being
        // resolvable in the test environment (resolve_command checks the real PATH/
        // filesystem) -- that path is exercised live in modules/mod.rs-external
        // testing instead. This confirms the diagnose()-level behavior is correct
        // regardless of what's installed on the machine running `cargo test`.
    }

    #[test]
    fn effective_only_stale_is_still_a_mismatch_when_bios_is_known_affected() {
        let mut data = base_data();
        // Source already declares it, but grub.cfg hasn't been regenerated yet.
        data.grub_cfg_declares_reboot_arg = Some(false);
        data.current_bios_is_known_affected = true;
        data.registry_match = Some(BoardCompatibilityEntry {
            vendor: "Micro-Star International".to_string(),
            name: "MAG X870E TOMAHAWK WIFI (MS-7E59)".to_string(),
            affected_bios_versions: vec!["2.AC3".to_string()],
            note: "reverts EXPO profile after flash".to_string(),
            recommended_reboot_mode: None,
        });
        let config = config_with_reboot_args("cold", "");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data));
        assert!(!diagnosis.compliant);
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|f| f.contains("does not reflect"))
        );
    }

    #[test]
    fn unreadable_grub_cfg_is_not_treated_as_a_mismatch() {
        let mut data = base_data();
        data.grub_cfg_declares_reboot_arg = None;
        data.observed_memory_gib = Some(64); // ensure mitigation IS needed here
        let config = config_with_memory_mismatch("cold", "", 128);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareReboot.diagnose(&ctx, &observation(data));
        assert!(
            !diagnosis
                .findings
                .iter()
                .any(|f| f.contains("does not reflect")),
            "None means \"can't confirm\", not \"wrong\" -- even when mitigation is needed"
        );
    }
}
