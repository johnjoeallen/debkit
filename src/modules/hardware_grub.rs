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
const GRUB_GFXMODE_KEY: &str = "GRUB_GFXMODE";
const GRUB_GFXPAYLOAD_LINUX_KEY: &str = "GRUB_GFXPAYLOAD_LINUX";
const GRUB_TIMEOUT_KEY: &str = "GRUB_TIMEOUT";

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
/// empty — see module doc) or locally-added registry at
/// `/etc/debkit/boards/registry.yaml`.
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
    /// `hardware_grub.reboot_mode` isn't explicitly set in config — an explicit
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
///
/// This module also manages the display resolution GRUB hands off at boot:
/// `GRUB_GFXMODE` (the GRUB menu's own resolution, before the kernel loads) and
/// `GRUB_GFXPAYLOAD_LINUX` (the resolution handed to the kernel/KMS when it boots),
/// both set from a single `video_mode` config value in the simple `<columns>x<rows>`
/// form (e.g. `"1024x768"`) rather than a raw VESA mode number. Unlike `reboot_mode`,
/// which is only ever written once one of the findings above is actually confirmed
/// (see `needs_grub_mitigation`), `video_mode` has no detection step — declaring it is
/// itself a direct, sufficient instruction to apply it. Unlike `reboot=`, these are
/// standalone `KEY=value` lines in `/etc/default/grub`, not tokens inside
/// `GRUB_CMDLINE_LINUX_DEFAULT`'s quoted value — see `set_grub_default_simple_key`.
///
/// Same standalone-line treatment applies to `GRUB_TIMEOUT` (`timeout` in config,
/// seconds the boot menu waits before booting the default entry): like `video_mode`,
/// declaring it is itself a direct, sufficient instruction to apply it, with no
/// detection step of its own.
///
/// All four writes (`reboot=`, `GRUB_GFXMODE`, `GRUB_GFXPAYLOAD_LINUX`,
/// `GRUB_TIMEOUT`) are still combined into a single `/etc/default/grub` `WriteFile`
/// per `plan()` run.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HardwareGrubObservation {
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
    /// Raw content of `/etc/default/grub`, if readable. `plan()` reads the actual
    /// patch off this rather than a precomputed field, since which of `reboot=`/
    /// `GRUB_GFXMODE`/`GRUB_GFXPAYLOAD_LINUX` need patching depends on `plan()`-level
    /// gating (see `needs_grub_mitigation` for `reboot=`; the other two are gated
    /// purely on whether `video_mode` is declared).
    grub_default_current: Option<String>,
    /// The effective `reboot=<mode>[,<type>]` token value (always computed, regardless
    /// of whether the mitigation is actually needed — see `needs_grub_mitigation`).
    reboot_desired_arg: String,
    /// Whether `GRUB_CMDLINE_LINUX_DEFAULT` already contains a `reboot=<desired>`
    /// token. `None` if the file couldn't be read, or didn't contain a recognizable
    /// `GRUB_CMDLINE_LINUX_DEFAULT="..."` line (this module never guesses at an
    /// unfamiliar layout).
    grub_default_declares_reboot_arg: Option<bool>,
    /// Whether `/boot/grub/grub.cfg` — the generated, effective config actually read
    /// at boot — already contains the desired `reboot=` token. `None` if unreadable
    /// (commonly root-only permissions; `debkit diagnose` run unprivileged can't
    /// confirm this).
    grub_cfg_declares_reboot_arg: Option<bool>,
    /// `Some(<columns>x<rows>)` when `hardware_grub.video_mode` is explicitly declared
    /// in config — unlike `reboot_desired_arg`, this has no fallback/registry default,
    /// so `None` here means "not requested," not "unknown."
    video_desired_mode: Option<String>,
    /// Whether `/etc/default/grub` already has an active `GRUB_GFXMODE=<mode>` line.
    /// `None` both when `video_desired_mode` is `None` (nothing to check) and when the
    /// file couldn't be read. Unlike `GRUB_CMDLINE_LINUX_DEFAULT`'s "refuse to guess"
    /// stance, a standalone `KEY=value` line has no ambiguity to guess around — absent
    /// just means absent.
    grub_default_declares_gfxmode: Option<bool>,
    /// Same shape as `grub_default_declares_gfxmode` but for `GRUB_GFXPAYLOAD_LINUX`.
    grub_default_declares_gfxpayload: Option<bool>,
    /// Whether the generated `/boot/grub/grub.cfg` reflects the desired
    /// `gfxmode=<mode>` setting. `None` both when `video_desired_mode` is `None` and
    /// when the file couldn't be read (commonly root-only permissions).
    grub_cfg_declares_gfxmode: Option<bool>,
    /// Same shape as `grub_cfg_declares_gfxmode` but for `gfxpayload=<mode>`.
    grub_cfg_declares_gfxpayload: Option<bool>,
    /// `Some(<seconds>)` when `hardware_grub.timeout` is explicitly declared in
    /// config — like `video_desired_mode`, this has no fallback/registry default, so
    /// `None` here means "not requested," not "unknown."
    timeout_desired: Option<u32>,
    /// Whether `/etc/default/grub` already has an active `GRUB_TIMEOUT=<seconds>`
    /// line. Same `None`-means-either-"nothing to check"-or-"unreadable" shape as
    /// `grub_default_declares_gfxmode`.
    grub_default_declares_timeout: Option<bool>,
    /// Whether the generated `/boot/grub/grub.cfg` reflects the desired
    /// `timeout=<seconds>` setting. Same shape as `grub_cfg_declares_gfxmode`.
    grub_cfg_declares_timeout: Option<bool>,
}

pub struct HardwareGrub;

impl Module for HardwareGrub {
    fn name(&self) -> &'static str {
        "hardware.grub"
    }

    fn config_key(&self) -> Option<&'static str> {
        Some("hardware_grub")
    }

    fn description(&self) -> &'static str {
        "AM5 board/BIOS identification, known-affected-BIOS registry, memory-capacity check, GRUB reboot=/GFXMODE/GFXPAYLOAD boot parameters"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let dmi = read_dmi();
        let registry = load_board_registry();
        let registry_match = dmi.find_registry_match(&registry);
        let current_bios_is_known_affected = registry_match
            .as_ref()
            .is_some_and(|entry| bios_version_is_listed(dmi.bios_version.as_deref(), entry));

        let effective_mode = effective_reboot_mode(
            &ctx.config.hardware_grub.reboot_mode,
            registry_match.as_ref(),
        );
        let reboot_desired_arg =
            desired_reboot_arg(&effective_mode, &ctx.config.hardware_grub.reboot_type);
        let reboot_token = format!("reboot={reboot_desired_arg}");
        let video_desired_mode = non_empty(&ctx.config.hardware_grub.video_mode);
        let timeout_desired = ctx.config.hardware_grub.timeout;

        let grub_default_current = fs::read_to_string(GRUB_DEFAULT_PATH).ok();
        let grub_default_raw_value = grub_default_current
            .as_deref()
            .and_then(grub_cmdline_default_raw_value);
        let grub_default_declares_reboot_arg = grub_default_raw_value
            .as_deref()
            .and_then(|value| grub_cmdline_declares(value, &reboot_token));
        let grub_default_declares_gfxmode = video_desired_mode.as_deref().and_then(|mode| {
            grub_default_current.as_deref().map(|content| {
                grub_default_simple_key_value(content, GRUB_GFXMODE_KEY).as_deref() == Some(mode)
            })
        });
        let grub_default_declares_gfxpayload = video_desired_mode.as_deref().and_then(|mode| {
            grub_default_current.as_deref().map(|content| {
                grub_default_simple_key_value(content, GRUB_GFXPAYLOAD_LINUX_KEY).as_deref()
                    == Some(mode)
            })
        });
        let grub_default_declares_timeout = timeout_desired.and_then(|seconds| {
            grub_default_current.as_deref().map(|content| {
                grub_default_simple_key_value(content, GRUB_TIMEOUT_KEY).as_deref()
                    == Some(seconds.to_string().as_str())
            })
        });

        let grub_cfg_content = read_grub_cfg();
        let grub_cfg_declares_reboot_arg = grub_cfg_content
            .as_deref()
            .map(|content| grub_cfg_declares_token(content, &reboot_token));
        let grub_cfg_declares_gfxmode = video_desired_mode.as_deref().and_then(|mode| {
            grub_cfg_content
                .as_deref()
                .map(|content| grub_cfg_declares_token(content, &format!("gfxmode={mode}")))
        });
        let grub_cfg_declares_gfxpayload = video_desired_mode.as_deref().and_then(|mode| {
            grub_cfg_content
                .as_deref()
                .map(|content| grub_cfg_declares_token(content, &format!("gfxpayload={mode}")))
        });
        let grub_cfg_declares_timeout = timeout_desired.and_then(|seconds| {
            grub_cfg_content
                .as_deref()
                .map(|content| grub_cfg_declares_token(content, &format!("timeout={seconds}")))
        });

        let data = HardwareGrubObservation {
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
            reboot_desired_arg,
            grub_default_declares_reboot_arg,
            grub_cfg_declares_reboot_arg,
            video_desired_mode,
            grub_default_declares_gfxmode,
            grub_default_declares_gfxpayload,
            grub_cfg_declares_gfxmode,
            grub_cfg_declares_gfxpayload,
            timeout_desired,
            grub_default_declares_timeout,
            grub_cfg_declares_timeout,
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
        if !ctx.config.hardware_grub.enabled {
            return Diagnosis::compliant();
        }

        let data: HardwareGrubObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read hardware.grub observation: {err}"
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

        let expected = ctx.config.hardware_grub.expected_memory_gib;
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

        if needs_grub_mitigation(ctx.config.hardware_grub.expected_memory_gib, &data) {
            let effective_mode = effective_reboot_mode(
                &ctx.config.hardware_grub.reboot_mode,
                data.registry_match.as_ref(),
            );
            let desired_arg =
                desired_reboot_arg(&effective_mode, &ctx.config.hardware_grub.reboot_type);
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
                    "{GRUB_CFG_PATH} does not reflect `reboot={desired_arg}` yet — run `update-grub` (or `debkit apply hardware.grub`)"
                ));
            }
        }

        // Unlike reboot_mode above, video_mode has no detection step of its own --
        // declaring it in config is itself a sufficient, direct instruction to apply
        // it (see the field's doc comment on HardwareGrubConfig).
        if let Some(mode) = &data.video_desired_mode {
            match data.grub_default_declares_gfxmode {
                Some(false) => findings.push(format!(
                    "{GRUB_DEFAULT_PATH} does not declare `{GRUB_GFXMODE_KEY}={mode}`"
                )),
                None => findings.push(format!(
                    "could not confirm whether {GRUB_DEFAULT_PATH} declares `{GRUB_GFXMODE_KEY}={mode}`"
                )),
                Some(true) => {}
            }
            match data.grub_default_declares_gfxpayload {
                Some(false) => findings.push(format!(
                    "{GRUB_DEFAULT_PATH} does not declare `{GRUB_GFXPAYLOAD_LINUX_KEY}={mode}`"
                )),
                None => findings.push(format!(
                    "could not confirm whether {GRUB_DEFAULT_PATH} declares `{GRUB_GFXPAYLOAD_LINUX_KEY}={mode}`"
                )),
                Some(true) => {}
            }
            if data.grub_cfg_declares_gfxmode == Some(false) {
                findings.push(format!(
                    "{GRUB_CFG_PATH} does not reflect `{GRUB_GFXMODE_KEY}={mode}` yet — run `update-grub` (or `debkit apply hardware.grub`)"
                ));
            }
            if data.grub_cfg_declares_gfxpayload == Some(false) {
                findings.push(format!(
                    "{GRUB_CFG_PATH} does not reflect `{GRUB_GFXPAYLOAD_LINUX_KEY}={mode}` yet — run `update-grub` (or `debkit apply hardware.grub`)"
                ));
            }
        }

        // Same as video_mode above: no detection step, declaring `timeout` in config
        // is itself a sufficient, direct instruction to apply it.
        if let Some(seconds) = data.timeout_desired {
            match data.grub_default_declares_timeout {
                Some(false) => findings.push(format!(
                    "{GRUB_DEFAULT_PATH} does not declare `{GRUB_TIMEOUT_KEY}={seconds}`"
                )),
                None => findings.push(format!(
                    "could not confirm whether {GRUB_DEFAULT_PATH} declares `{GRUB_TIMEOUT_KEY}={seconds}`"
                )),
                Some(true) => {}
            }
            if data.grub_cfg_declares_timeout == Some(false) {
                findings.push(format!(
                    "{GRUB_CFG_PATH} does not reflect `{GRUB_TIMEOUT_KEY}={seconds}` yet — run `update-grub` (or `debkit apply hardware.grub`)"
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
        if !ctx.config.hardware_grub.enabled || diagnosis.compliant {
            return Ok(plan);
        }

        let data: HardwareGrubObservation = serde_json::from_value(observation.data.clone())?;

        // See module doc comment: the memory-capacity and known-affected-BIOS
        // findings themselves have no automated fix. reboot= is only ever patched
        // when one of them is confirmed -- not just because reboot_mode/reboot_type
        // are declared. A passing memory check and no BIOS-registry match means
        // reboot= is left alone entirely, even if it doesn't already say reboot=cold.
        // video_mode has no such detection step: declaring it is itself the
        // instruction to apply it (see the diagnose() comment above).
        let reboot_active =
            needs_grub_mitigation(ctx.config.hardware_grub.expected_memory_gib, &data);

        let reboot_needs_write =
            reboot_active && data.grub_default_declares_reboot_arg == Some(false);
        let gfxmode_needs_write =
            data.video_desired_mode.is_some() && data.grub_default_declares_gfxmode == Some(false);
        let gfxpayload_needs_write = data.video_desired_mode.is_some()
            && data.grub_default_declares_gfxpayload == Some(false);
        let timeout_needs_write =
            data.timeout_desired.is_some() && data.grub_default_declares_timeout == Some(false);

        let effective_stale = (reboot_active && data.grub_cfg_declares_reboot_arg == Some(false))
            || (data.video_desired_mode.is_some()
                && (data.grub_cfg_declares_gfxmode == Some(false)
                    || data.grub_cfg_declares_gfxpayload == Some(false)))
            || (data.timeout_desired.is_some() && data.grub_cfg_declares_timeout == Some(false));

        let any_default_write_needed = reboot_needs_write
            || gfxmode_needs_write
            || gfxpayload_needs_write
            || timeout_needs_write;
        if !any_default_write_needed && !effective_stale {
            return Ok(plan);
        }

        let Some(update_grub) =
            resolve_command(&["/usr/sbin/update-grub", "/sbin/update-grub", "update-grub"])
        else {
            // No update-grub on this host -- not a GRUB-managed boot setup DebKit
            // can act on. Writing /etc/default/grub with nothing to regenerate it
            // would just leave a half-applied, misleading state.
            return Ok(plan);
        };

        if any_default_write_needed && let Some(current) = data.grub_default_current.as_deref() {
            // reboot= is a token inside GRUB_CMDLINE_LINUX_DEFAULT's quoted value;
            // GRUB_GFXMODE/GRUB_GFXPAYLOAD_LINUX are their own standalone `KEY=value`
            // lines. Both kinds of edit still land in a single /etc/default/grub
            // WriteFile.
            let mut content = current.to_string();
            let mut declared: Vec<String> = Vec::new();
            if reboot_needs_write {
                let token = format!("reboot={}", data.reboot_desired_arg);
                if let Some(patched) = patch_grub_cmdline_default(&content, "reboot=", &token) {
                    content = patched;
                    declared.push(format!("`{token}`"));
                }
            }
            if gfxmode_needs_write {
                let mode = data
                    .video_desired_mode
                    .as_deref()
                    .expect("gfxmode_needs_write implies video_desired_mode is Some");
                content = set_grub_default_simple_key(&content, GRUB_GFXMODE_KEY, mode);
                declared.push(format!("`{GRUB_GFXMODE_KEY}={mode}`"));
            }
            if gfxpayload_needs_write {
                let mode = data
                    .video_desired_mode
                    .as_deref()
                    .expect("gfxpayload_needs_write implies video_desired_mode is Some");
                content = set_grub_default_simple_key(&content, GRUB_GFXPAYLOAD_LINUX_KEY, mode);
                declared.push(format!("`{GRUB_GFXPAYLOAD_LINUX_KEY}={mode}`"));
            }
            if timeout_needs_write {
                let seconds = data
                    .timeout_desired
                    .expect("timeout_needs_write implies timeout_desired is Some");
                content =
                    set_grub_default_simple_key(&content, GRUB_TIMEOUT_KEY, &seconds.to_string());
                declared.push(format!("`{GRUB_TIMEOUT_KEY}={seconds}`"));
            }
            if !declared.is_empty() {
                plan.push(
                    format!("declare {} in {GRUB_DEFAULT_PATH}", declared.join(", ")),
                    Risk::High,
                    Change::WriteFile {
                        path: PathBuf::from(GRUB_DEFAULT_PATH),
                        content,
                    },
                );
            }
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
        if !ctx.config.hardware_grub.enabled {
            return Ok(vec![VerificationResult::skipped(
                "hardware.grub",
                "disabled in config",
            )]);
        }

        let mut checks = Vec::new();

        let expected = ctx.config.hardware_grub.expected_memory_gib;
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
            &ctx.config.hardware_grub.reboot_mode,
            registry_match.as_ref(),
        );
        let desired_arg =
            desired_reboot_arg(&effective_mode, &ctx.config.hardware_grub.reboot_type);
        let reboot_token = format!("reboot={desired_arg}");
        let check_name = format!("{GRUB_CFG_PATH} declares {reboot_token}");
        let grub_cfg_content = read_grub_cfg();
        match grub_cfg_content
            .as_deref()
            .map(|content| grub_cfg_declares_token(content, &reboot_token))
        {
            Some(true) => checks.push(VerificationResult::pass(check_name)),
            Some(false) => checks.push(VerificationResult::fail(
                check_name,
                "not present in the generated config — run `update-grub` or `debkit apply hardware.grub`",
            )),
            None => checks.push(VerificationResult::skipped(
                check_name,
                "could not read /boot/grub/grub.cfg (commonly root-only permissions)",
            )),
        }

        match non_empty(&ctx.config.hardware_grub.video_mode) {
            Some(mode) => {
                for (key, label) in [
                    (GRUB_GFXMODE_KEY, "gfxmode"),
                    (GRUB_GFXPAYLOAD_LINUX_KEY, "gfxpayload"),
                ] {
                    let token = format!("{label}={mode}");
                    let check_name = format!("{GRUB_CFG_PATH} declares {key}={mode}");
                    match grub_cfg_content
                        .as_deref()
                        .map(|content| grub_cfg_declares_token(content, &token))
                    {
                        Some(true) => checks.push(VerificationResult::pass(check_name)),
                        Some(false) => checks.push(VerificationResult::fail(
                            check_name,
                            "not present in the generated config — run `update-grub` or `debkit apply hardware.grub`",
                        )),
                        None => checks.push(VerificationResult::skipped(
                            check_name,
                            "could not read /boot/grub/grub.cfg (commonly root-only permissions)",
                        )),
                    }
                }
            }
            None => checks.push(VerificationResult::skipped(
                format!("{GRUB_CFG_PATH} declares {GRUB_GFXMODE_KEY}/{GRUB_GFXPAYLOAD_LINUX_KEY}"),
                "`video_mode` not declared",
            )),
        }

        match ctx.config.hardware_grub.timeout {
            Some(seconds) => {
                let token = format!("timeout={seconds}");
                let check_name = format!("{GRUB_CFG_PATH} declares {GRUB_TIMEOUT_KEY}={seconds}");
                match grub_cfg_content
                    .as_deref()
                    .map(|content| grub_cfg_declares_token(content, &token))
                {
                    Some(true) => checks.push(VerificationResult::pass(check_name)),
                    Some(false) => checks.push(VerificationResult::fail(
                        check_name,
                        "not present in the generated config — run `update-grub` or `debkit apply hardware.grub`",
                    )),
                    None => checks.push(VerificationResult::skipped(
                        check_name,
                        "could not read /boot/grub/grub.cfg (commonly root-only permissions)",
                    )),
                }
            }
            None => checks.push(VerificationResult::skipped(
                format!("{GRUB_CFG_PATH} declares {GRUB_TIMEOUT_KEY}"),
                "`timeout` not declared",
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
/// `hardware_grub.reboot_mode` in config always wins; otherwise falls back to the
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
fn needs_grub_mitigation(expected_memory_gib: u32, data: &HardwareGrubObservation) -> bool {
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

/// Whether the quoted value already contains `desired_token` (e.g. `"reboot=cold"`)
/// as a whole token, anywhere in the value (not just at the end — see
/// `patch_quoted_value`'s doc comment for why position matters). `None` if `value`
/// isn't a simple `"..."`/`'...'` quoted string.
fn grub_cmdline_declares(value: &str, desired_token: &str) -> Option<bool> {
    let inner = unquote(value)?;
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

/// Replaces any existing token starting with `prefix` (e.g. `"reboot="`) in `value`'s
/// quoted contents with `desired_token` (e.g. `"reboot=cold"`) — appending it if none
/// was present — and re-quotes with the same quote character. Deliberately does NOT
/// try to preserve the original token's position — it filters out any existing token
/// with that prefix and appends the new one at the end. That's fine for
/// `grub_cmdline_declares`'s "is it there at all" check (position-independent), but
/// means `patch_grub_cmdline_default`'s output can differ byte-for-byte from an
/// already-compliant file if the existing token wasn't already last; callers must use
/// the semantic `grub_cmdline_declares` check to decide whether a write is needed at
/// all, never raw string equality.
fn patch_quoted_value(value: &str, prefix: &str, desired_token: &str) -> Option<String> {
    let quote = value.trim_end().chars().next()?;
    let inner = unquote(value)?;
    let mut tokens: Vec<String> = inner
        .split_whitespace()
        .filter(|token| !token.starts_with(prefix))
        .map(str::to_string)
        .collect();
    tokens.push(desired_token.to_string());
    Some(format!("{quote}{}{quote}", tokens.join(" ")))
}

/// Patches `/etc/default/grub`'s full content, replacing only the
/// `GRUB_CMDLINE_LINUX_DEFAULT=` line's value and leaving every other line — including
/// a separate active `GRUB_CMDLINE_LINUX=` line, comments, blank lines — untouched.
/// `reboot=` is currently the only token this module manages inside that quoted
/// value (`video_mode` sets standalone `GRUB_GFXMODE`/`GRUB_GFXPAYLOAD_LINUX` lines
/// instead — see `set_grub_default_simple_key`). Returns `None` if no matching line
/// was found (this module never guesses at an unfamiliar grub layout) or the line's
/// value wasn't a simple quoted string.
fn patch_grub_cmdline_default(current: &str, prefix: &str, desired_token: &str) -> Option<String> {
    let mut found = false;
    let mut patched_lines: Vec<String> = Vec::new();
    for line in current.lines() {
        let trimmed = line.trim_start();
        if !found
            && !trimmed.starts_with('#')
            && let Some(rest) = trimmed.strip_prefix(GRUB_CMDLINE_KEY)
            && let Some(value) = rest.strip_prefix('=')
            && let Some(patched_value) = patch_quoted_value(value, prefix, desired_token)
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

/// Returns the active (non-commented) value of a standalone `KEY=value` line in
/// `/etc/default/grub`, unquoted if it was quoted -- used for `GRUB_GFXMODE`/
/// `GRUB_GFXPAYLOAD_LINUX`, which (unlike `GRUB_CMDLINE_LINUX_DEFAULT`) are simple
/// whole-line settings, not tokens inside a quoted multi-value string. `None` if no
/// such active line exists, which is a normal "not declared yet" state -- Debian's
/// stock `/etc/default/grub` template ships these commented out or omits them
/// entirely, so absence carries none of `GRUB_CMDLINE_LINUX_DEFAULT`'s "unfamiliar
/// layout, refuse to guess" ambiguity.
fn grub_default_simple_key_value(content: &str, key: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            return None;
        }
        let value = trimmed.strip_prefix(key)?.strip_prefix('=')?.trim_end();
        Some(unquote(value).unwrap_or(value).to_string())
    })
}

/// Sets `key=value` as a standalone line in `/etc/default/grub`'s content: replaces
/// the value of an existing active (non-commented) `key=` line if one exists, written
/// unquoted (our values are always a plain `<columns>x<rows>`, never containing
/// whitespace or shell metacharacters -- see `config::is_valid_video_mode`), or
/// appends a new `key=value` line at the end if no active line exists yet. Unlike
/// `patch_grub_cmdline_default` (which refuses to guess at an unfamiliar
/// `GRUB_CMDLINE_LINUX_DEFAULT` layout), appending a brand new standalone line here
/// carries none of that ambiguity.
fn set_grub_default_simple_key(current: &str, key: &str, value: &str) -> String {
    let desired_line = format!("{key}={value}");
    let mut found = false;
    let mut lines: Vec<String> = Vec::new();
    for line in current.lines() {
        let trimmed = line.trim_start();
        if !found
            && !trimmed.starts_with('#')
            && trimmed
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
                .is_some()
        {
            found = true;
            lines.push(desired_line.clone());
            continue;
        }
        lines.push(line.to_string());
    }
    if !found {
        lines.push(desired_line);
    }
    let mut result = lines.join("\n");
    if (!found || current.ends_with('\n')) && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn read_grub_cfg() -> Option<String> {
    fs::read_to_string(GRUB_CFG_PATH).ok()
}

/// Whether `desired_token` (e.g. `"reboot=cold"`, `"gfxmode=1024x768"`) appears
/// anywhere in the generated `grub.cfg` (across any boot entry) — a coarse but real
/// functional signal, not a per-entry guarantee.
fn grub_cfg_declares_token(grub_cfg: &str, desired_token: &str) -> bool {
    grub_cfg
        .lines()
        .any(|line| line.split_whitespace().any(|token| token == desired_token))
}

/// `None` for an empty/whitespace-only string, `Some(trimmed)` otherwise — used for
/// config fields like `video_mode` that have no fallback default, where empty
/// specifically means "not declared."
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// `update-grub` lives in `/usr/sbin`, which typically isn't on a regular user's
/// `PATH` on Debian — same footgun `network.firewall` hit with `iptables`/`nft`.
/// Callers should list well-known absolute paths *before* the bare name: a bare-name
/// `PATH` lookup can resolve differently depending on who's running it (this module's
/// own `--dry-run`/`--from-run` pair is a real example — staging typically runs
/// unprivileged, with a `PATH` that excludes `/usr/sbin`, while promoting typically
/// runs under `sudo`, whose `secure_path` includes it), which would make the exact
/// same real command resolve to two different `Change` values and spuriously fail
/// the plan-freshness check `--from-run` relies on. Absolute-path candidates are a
/// plain filesystem check, so they resolve identically no matter who's asking.
fn resolve_command(candidates: &[&'static str]) -> Option<&'static str> {
    candidates.iter().copied().find(|candidate| {
        if candidate.starts_with('/') {
            Path::new(candidate).exists()
        } else {
            exec::command_available(candidate)
        }
    })
}

/// Installed by the .deb package (see `Cargo.toml`'s `[package.metadata.deb]` assets
/// and `data/boards/registry.yaml` in the repo) — ships empty deliberately, same
/// reasoning as `embedded_board_registry`. A real entry a user verifies belongs in a
/// PR to that file so every user with the same board benefits; a local-only entry
/// belongs in `LOCAL_BOARD_REGISTRY_PATH` instead, which is merged on top of this one
/// and always wins on a conflict.
const SYSTEM_BOARD_REGISTRY_PATH: &str = "/usr/share/debkit/boards/registry.yaml";

/// The locally-editable tier, also packaged by the `.deb` (as a dpkg conffile, from
/// the same `data/boards/registry.yaml` source as `SYSTEM_BOARD_REGISTRY_PATH` — see
/// `Cargo.toml`), and always wins on a matching vendor+name conflict. Deliberately
/// system-wide rather than per-user: which BIOS versions are known-bad on a given
/// board is a property of the machine, not of whichever Unix user happens to run
/// `debkit`.
const LOCAL_BOARD_REGISTRY_PATH: &str = "/etc/debkit/boards/registry.yaml";

/// Currently empty: this plan deliberately doesn't ship fabricated known-affected-BIOS
/// data (vendor compatibility notes are the kind of thing that's easy to get wrong and
/// costly to trust incorrectly). The mechanism is real and ready — see
/// `SYSTEM_BOARD_REGISTRY_PATH` (packaged defaults) and `LOCAL_BOARD_REGISTRY_PATH`
/// (local additions).
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
/// (empty) default, the .deb-packaged system registry, then the locally-editable
/// `/etc/debkit/boards/registry.yaml`.
fn load_board_registry() -> Vec<BoardCompatibilityEntry> {
    let registry = merge_board_registry(
        embedded_board_registry(),
        Path::new(SYSTEM_BOARD_REGISTRY_PATH),
    );
    merge_board_registry(registry, Path::new(LOCAL_BOARD_REGISTRY_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(HardwareGrub.name(), "hardware.grub");
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
            "debkit_hardware_grub_test_{}_{}",
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

    /// The packaged system registry ships defaults; the locally-editable file
    /// overrides them on a matching vendor+name, exactly like `load_board_registry`'s
    /// three-tier chain (embedded -> system -> local) is meant to behave.
    #[test]
    fn merge_board_registry_lets_a_later_tier_override_an_earlier_one() {
        let dir = std::env::temp_dir().join(format!(
            "debkit_hardware_grub_tier_test_{}_{}",
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

    fn observation(data: HardwareGrubObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn base_data() -> HardwareGrubObservation {
        HardwareGrubObservation {
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
            reboot_desired_arg: "cold".to_string(),
            grub_default_declares_reboot_arg: Some(true),
            grub_cfg_declares_reboot_arg: Some(true),
            video_desired_mode: None,
            grub_default_declares_gfxmode: None,
            grub_default_declares_gfxpayload: None,
            grub_cfg_declares_gfxmode: None,
            grub_cfg_declares_gfxpayload: None,
            timeout_desired: None,
            grub_default_declares_timeout: None,
            grub_cfg_declares_timeout: None,
        }
    }

    fn config(enabled: bool) -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.hardware_grub.enabled = enabled;
        config
    }

    #[test]
    fn compliant_when_nothing_flagged() {
        let config = config(true);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
        let plan = HardwareGrub
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
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        assert!(diagnosis.findings[0].contains("known-affected version"));
        let plan = HardwareGrub
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty(), "no automated fix exists for this finding");
    }

    #[test]
    fn mismatch_when_memory_capacity_drifts_from_expected() {
        let mut data = base_data();
        data.observed_memory_gib = Some(64);
        let mut config = config(true);
        config.hardware_grub.expected_memory_gib = 128;
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
        assert!(!diagnosis.compliant);
        assert!(diagnosis.findings[0].contains("does not match declared"));
    }

    #[test]
    fn compliant_when_memory_matches_expected() {
        let mut config = config(true);
        config.hardware_grub.expected_memory_gib = 128;
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
    }

    #[test]
    fn expected_memory_zero_skips_the_check() {
        let mut data = base_data();
        data.observed_memory_gib = Some(64);
        let config = config(true);
        assert_eq!(config.hardware_grub.expected_memory_gib, 0);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
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
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
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
        assert_eq!(
            grub_cmdline_declares("\"reboot=cold\"", "reboot=cold"),
            Some(true)
        );
        assert_eq!(
            grub_cmdline_declares("\"quiet reboot=cold splash\"", "reboot=cold"),
            Some(true)
        );
        assert_eq!(
            grub_cmdline_declares("\"reboot=warm\"", "reboot=cold"),
            Some(false)
        );
        // Not a substring match -- "reboot=cold,acpi" must not satisfy "reboot=cold".
        assert_eq!(
            grub_cmdline_declares("\"reboot=cold,acpi\"", "reboot=cold"),
            Some(false)
        );
        assert_eq!(grub_cmdline_declares("unquoted", "reboot=cold"), None);
    }

    #[test]
    fn patch_quoted_value_replaces_existing_token_with_matching_prefix() {
        assert_eq!(
            patch_quoted_value("\"quiet reboot=warm splash\"", "reboot=", "reboot=cold"),
            Some("\"quiet splash reboot=cold\"".to_string())
        );
    }

    #[test]
    fn patch_quoted_value_appends_when_absent() {
        assert_eq!(
            patch_quoted_value("\"quiet\"", "reboot=", "reboot=cold"),
            Some("\"quiet reboot=cold\"".to_string())
        );
    }

    #[test]
    fn patch_quoted_value_only_touches_tokens_with_the_given_prefix() {
        // A "foo=" patch must leave an unrelated "reboot=" token alone.
        assert_eq!(
            patch_quoted_value("\"quiet reboot=cold\"", "foo=", "foo=bar"),
            Some("\"quiet reboot=cold foo=bar\"".to_string())
        );
    }

    #[test]
    fn patch_grub_cmdline_default_only_touches_the_default_line() {
        let patched =
            patch_grub_cmdline_default(REAL_GRUB_DEFAULT_SAMPLE, "reboot=", "reboot=cold,acpi")
                .unwrap();
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
        assert_eq!(
            patch_grub_cmdline_default("GRUB_TIMEOUT=5\n", "reboot=", "reboot=cold"),
            None
        );
    }

    #[test]
    fn grub_cfg_token_check_is_a_substring_of_lines_not_whole_content() {
        let cfg = "linux /boot/vmlinuz root=UUID=x ro quiet reboot=cold\ninitrd /boot/initrd.img\n";
        assert!(grub_cfg_declares_token(cfg, "reboot=cold"));
        assert!(!grub_cfg_declares_token(cfg, "reboot=warm"));
    }

    #[test]
    fn non_empty_trims_and_treats_blank_as_none() {
        assert_eq!(non_empty(""), None);
        assert_eq!(non_empty("   "), None);
        assert_eq!(non_empty(" 791 "), Some("791".to_string()));
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
        config.hardware_grub.enabled = true;
        config.hardware_grub.reboot_mode = mode.to_string();
        config.hardware_grub.reboot_type = kind.to_string();
        config
    }

    fn config_with_memory_mismatch(
        mode: &str,
        kind: &str,
        expected_memory_gib: u32,
    ) -> crate::config::DebkitConfig {
        let mut config = config_with_reboot_args(mode, kind);
        config.hardware_grub.expected_memory_gib = expected_memory_gib;
        config
    }

    #[test]
    fn compliant_when_grub_already_declares_desired_arg() {
        let config = config_with_reboot_args("cold", "");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(base_data()));
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
        data.grub_cfg_declares_reboot_arg = Some(false);
        data.observed_memory_gib = Some(128); // matches expected_memory_gib below
        data.current_bios_is_known_affected = false;

        let config = config_with_memory_mismatch("cold", "", 128);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data.clone()));
        assert!(diagnosis.compliant);
        let plan = HardwareGrub
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
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
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
        data.grub_cfg_declares_reboot_arg = Some(false);
        data.observed_memory_gib = Some(128); // matches expected_memory_gib below
        data.current_bios_is_known_affected = false;
        data.registry_match = Some(registry_entry_with_recommendation("cold"));

        let config = config_with_memory_mismatch("", "", 128);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = HardwareGrub
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
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    #[test]
    fn mismatch_and_plan_when_grub_default_missing_the_arg_and_memory_mismatched() {
        let mut data = base_data();
        data.grub_default_current = Some("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"\n".to_string());
        data.grub_default_declares_reboot_arg = Some(false);
        data.grub_cfg_declares_reboot_arg = Some(false);
        data.observed_memory_gib = Some(64); // a DIMM went undetected

        let config = config_with_memory_mismatch("cold", "", 128);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data.clone()));
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
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
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
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
        assert!(
            !diagnosis
                .findings
                .iter()
                .any(|f| f.contains("does not reflect")),
            "None means \"can't confirm\", not \"wrong\" -- even when mitigation is needed"
        );
    }

    fn config_with_video_mode(mode: &str) -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.hardware_grub.enabled = true;
        config.hardware_grub.video_mode = mode.to_string();
        config
    }

    /// Unlike reboot_mode, declaring video_mode is by itself a sufficient reason to
    /// touch grub -- there's no confirmed-evidence gate the way `needs_grub_mitigation`
    /// enforces for reboot=. No memory mismatch, no BIOS match, no registry
    /// recommendation here; video_mode alone should still produce a mismatch + plan,
    /// for both GRUB_GFXMODE and GRUB_GFXPAYLOAD_LINUX independently.
    #[test]
    fn mismatch_when_video_mode_declared_but_not_present_in_grub_default() {
        let mut data = base_data();
        data.grub_default_current = Some("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"\n".to_string());
        data.video_desired_mode = Some("1024x768".to_string());
        data.grub_default_declares_gfxmode = Some(false);
        data.grub_default_declares_gfxpayload = Some(false);
        data.grub_cfg_declares_gfxmode = Some(false);
        data.grub_cfg_declares_gfxpayload = Some(false);

        let config = config_with_video_mode("1024x768");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|f| f.contains("does not declare `GRUB_GFXMODE=1024x768`"))
        );
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|f| f.contains("does not declare `GRUB_GFXPAYLOAD_LINUX=1024x768`"))
        );
    }

    #[test]
    fn compliant_when_grub_already_declares_the_desired_video_mode() {
        let mut data = base_data();
        data.video_desired_mode = Some("1024x768".to_string());
        data.grub_default_declares_gfxmode = Some(true);
        data.grub_default_declares_gfxpayload = Some(true);
        data.grub_cfg_declares_gfxmode = Some(true);
        data.grub_cfg_declares_gfxpayload = Some(true);

        let config = config_with_video_mode("1024x768");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    /// video_mode not declared at all means the observation carries no
    /// video_desired_mode -- no finding, no plan, regardless of what grub happens to
    /// contain.
    #[test]
    fn video_mode_undeclared_produces_no_findings() {
        let data = base_data();
        assert_eq!(data.video_desired_mode, None);
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    fn config_with_timeout(seconds: u32) -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.hardware_grub.enabled = true;
        config.hardware_grub.timeout = Some(seconds);
        config
    }

    /// Same shape as video_mode's mismatch test: declaring `timeout` is by itself
    /// sufficient to touch grub, no confirmed-evidence gate required.
    #[test]
    fn mismatch_when_timeout_declared_but_not_present_in_grub_default() {
        let mut data = base_data();
        data.grub_default_current = Some("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet\"\n".to_string());
        data.timeout_desired = Some(15);
        data.grub_default_declares_timeout = Some(false);
        data.grub_cfg_declares_timeout = Some(false);

        let config = config_with_timeout(15);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|f| f.contains("does not declare `GRUB_TIMEOUT=15`"))
        );

        let plan = HardwareGrub
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(
            plan.changes
                .iter()
                .any(|change| change.description.contains("GRUB_TIMEOUT=15"))
        );
    }

    #[test]
    fn compliant_when_grub_already_declares_the_desired_timeout() {
        let mut data = base_data();
        data.timeout_desired = Some(15);
        data.grub_default_declares_timeout = Some(true);
        data.grub_cfg_declares_timeout = Some(true);

        let config = config_with_timeout(15);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    /// `timeout: None` (the default) means no finding, no plan, regardless of what
    /// grub happens to contain -- same as video_mode when undeclared.
    #[test]
    fn timeout_undeclared_produces_no_findings() {
        let data = base_data();
        assert_eq!(data.timeout_desired, None);
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    /// `Some(0)` is a legitimate, distinct value (skip the grub menu delay) -- it
    /// must not be treated the same as "not declared".
    #[test]
    fn timeout_zero_is_a_declared_value_not_unset() {
        let mut data = base_data();
        data.timeout_desired = Some(0);
        data.grub_default_declares_timeout = Some(false);
        data.grub_cfg_declares_timeout = Some(false);

        let config = config_with_timeout(0);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareGrub.diagnose(&ctx, &observation(data));
        assert!(!diagnosis.compliant);
        assert!(
            diagnosis
                .findings
                .iter()
                .any(|f| f.contains("does not declare `GRUB_TIMEOUT=0`"))
        );
    }

    #[test]
    fn grub_default_simple_key_value_finds_active_line_ignoring_commented() {
        let content = "#GRUB_GFXMODE=640x480\nGRUB_GFXMODE=1024x768\n";
        assert_eq!(
            grub_default_simple_key_value(content, GRUB_GFXMODE_KEY),
            Some("1024x768".to_string())
        );
    }

    #[test]
    fn grub_default_simple_key_value_unquotes_when_quoted() {
        let content = "GRUB_GFXPAYLOAD_LINUX=\"1024x768\"\n";
        assert_eq!(
            grub_default_simple_key_value(content, GRUB_GFXPAYLOAD_LINUX_KEY),
            Some("1024x768".to_string())
        );
    }

    #[test]
    fn grub_default_simple_key_value_none_when_absent() {
        let content = "GRUB_TIMEOUT=5\n";
        assert_eq!(
            grub_default_simple_key_value(content, GRUB_GFXMODE_KEY),
            None
        );
    }

    #[test]
    fn set_grub_default_simple_key_replaces_existing_active_line() {
        let current = "GRUB_TIMEOUT=5\nGRUB_GFXMODE=640x480\nGRUB_DEFAULT=0\n";
        let updated = set_grub_default_simple_key(current, GRUB_GFXMODE_KEY, "1024x768");
        assert!(updated.contains("GRUB_GFXMODE=1024x768\n"));
        assert!(!updated.contains("640x480"));
        // Unrelated lines survive untouched, in their original order.
        assert!(updated.contains("GRUB_TIMEOUT=5\n"));
        assert!(updated.contains("GRUB_DEFAULT=0\n"));
    }

    #[test]
    fn set_grub_default_simple_key_appends_when_absent() {
        let current = "GRUB_TIMEOUT=5\n";
        let updated = set_grub_default_simple_key(current, GRUB_GFXMODE_KEY, "1024x768");
        assert_eq!(updated, "GRUB_TIMEOUT=5\nGRUB_GFXMODE=1024x768\n");
    }

    /// A commented example line (Debian's stock template ships `#GRUB_GFXMODE=...`)
    /// must be left alone, not mistaken for an active declaration to replace.
    #[test]
    fn set_grub_default_simple_key_ignores_a_commented_example_line() {
        let current = "#GRUB_GFXMODE=640x480\nGRUB_TIMEOUT=5\n";
        let updated = set_grub_default_simple_key(current, GRUB_GFXMODE_KEY, "1024x768");
        assert!(updated.contains("#GRUB_GFXMODE=640x480\n"));
        assert!(updated.contains("GRUB_GFXMODE=1024x768\n"));
    }

    #[test]
    fn set_grub_default_simple_key_appends_a_trailing_newline_even_without_one_in_source() {
        let current = "GRUB_TIMEOUT=5";
        let updated = set_grub_default_simple_key(current, GRUB_GFXMODE_KEY, "1024x768");
        assert_eq!(updated, "GRUB_TIMEOUT=5\nGRUB_GFXMODE=1024x768\n");
    }
}
