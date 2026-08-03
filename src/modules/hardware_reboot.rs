use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::ChangePlan;

const DMI_ROOT: &str = "/sys/class/dmi/id";

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
/// `plan()` is always empty: there is no automated fix for "your BIOS is on a
/// known-affected version" or "a DIMM went undetected" — this module is purely
/// diagnostic, matching `systemd.units`/`identity.nis`'s uninitialized-maps precedent
/// for standing findings this codebase doesn't attempt to auto-close.
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
}

pub struct HardwareReboot;

impl Module for HardwareReboot {
    fn name(&self) -> &'static str {
        "hardware.reboot"
    }

    fn discover(&self, _ctx: &Context) -> anyhow::Result<Observation> {
        let dmi = read_dmi();
        let registry = load_board_registry();
        let registry_match = dmi.find_registry_match(&registry);
        let current_bios_is_known_affected = registry_match
            .as_ref()
            .is_some_and(|entry| bios_version_is_listed(dmi.bios_version.as_deref(), entry));

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

        if findings.is_empty() {
            Diagnosis::compliant()
        } else {
            Diagnosis::mismatch(findings)
        }
    }

    fn plan(
        &self,
        _ctx: &Context,
        _observation: &Observation,
        _diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        // See module doc comment: neither a BIOS compatibility flag nor an undetected
        // DIMM has an automated fix. Purely diagnostic.
        Ok(ChangePlan::new())
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
        match dmi.find_registry_match(&registry) {
            Some(entry) if bios_version_is_listed(dmi.bios_version.as_deref(), &entry) => {
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

fn board_registry_path(home: &Path) -> PathBuf {
    home.join(".config")
        .join("debkit")
        .join("boards")
        .join("registry.yaml")
}

/// Currently empty: this plan deliberately doesn't ship fabricated known-affected-BIOS
/// data (vendor compatibility notes are the kind of thing that's easy to get wrong and
/// costly to trust incorrectly). The mechanism is real and ready; a user who hits a
/// specific known-bad BIOS version records it in
/// `~/.config/debkit/boards/registry.yaml`.
fn embedded_board_registry() -> Vec<BoardCompatibilityEntry> {
    Vec::new()
}

fn load_board_registry_from_path(path: &Path) -> Vec<BoardCompatibilityEntry> {
    let mut registry = embedded_board_registry();
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

fn load_board_registry() -> Vec<BoardCompatibilityEntry> {
    match crate::config::home_dir() {
        Ok(home) => load_board_registry_from_path(&board_registry_path(&home)),
        Err(_) => embedded_board_registry(),
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

        let registry = load_board_registry_from_path(&path);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry[0].vendor, "Micro-Star International");
        assert_eq!(registry[0].affected_bios_versions, vec!["2.AC3"]);
    }

    #[test]
    fn registry_from_missing_path_falls_back_to_embedded() {
        let registry = load_board_registry_from_path(Path::new("/nonexistent/registry.yaml"));
        assert_eq!(registry, embedded_board_registry());
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
}
