use std::fs;
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::{Change, ChangePlan, Risk};

const MODULES_LOAD_CONF_PATH: &str = "/etc/modules-load.d/debkit-i2c-dev.conf";
const OPENRGB_SDK_ADDR: &str = "127.0.0.1:6742";
const UDEV_RULE_DIRS: &[&str] = &[
    "/etc/udev/rules.d",
    "/usr/lib/udev/rules.d",
    "/lib/udev/rules.d",
];

/// Manages exactly one thing declaratively: the `i2c-dev` kernel module (loaded now,
/// and declared to load on every future boot via `/etc/modules-load.d/`), because that
/// is the specific, real, currently-reproducible prerequisite gap this dev host has —
/// `i2c-piix4` registers the AMD SMBus adapter, but without `i2c-dev` no `/dev/i2c-*`
/// character devices exist for userspace (OpenRGB or anything else) to open, so
/// motherboard RGB control silently doesn't work with no obvious error message
/// anywhere. Loading a kernel module is a standard, low-risk, reversible (`rmmod`)
/// operation — a different risk profile from writing firewall/DNS/bootloader state.
///
/// Everything else about RGB control is deliberately out of scope for `plan()`/`apply()`:
/// - This module never executes the `openrgb` binary itself. Its runtime behavior
///   against real hardware (device/bus scanning) can't be verified in this environment,
///   and the shared `engine::exec` layer has no command-timeout primitive to safely
///   bound a call that might hang scanning an SMBus. `openrgb_installed` is checked via
///   `command -v` only; nothing that would exec the binary and touch hardware.
/// - There's no reliable way to read back "what color is this LED right now" to
///   converge against, unlike a config file's content or a NIC's driver setting — so
///   device enumeration, color state, and lighting profiles aren't modeled here at all.
///   Re-implementing OpenRGB's own hardware-ID database would also just duplicate a
///   project that already does this well.
/// - A missing OpenRGB udev rule (needed for non-root access to USB-connected RGB
///   controllers) is surfaced as a warning, not auto-fixed: the correct rule/group
///   varies by distro and by what OpenRGB's own packaging already provides, and
///   getting it wrong risks granting broader device access than intended.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RgbObservation {
    openrgb_installed: bool,
    sdk_server_reachable: bool,
    i2c_dev_loaded: bool,
    i2c_related_modules: Vec<String>,
    i2c_device_nodes: Vec<String>,
    /// `None` when no `/dev/i2c-*` node exists to test; otherwise whether the first one
    /// could be opened for read+write by the invoking user (a real, if narrow, signal
    /// for "is device permission/group setup correct").
    i2c_device_node_writable: Option<bool>,
    modules_load_conf_current: Option<String>,
    udev_rule_present: bool,
}

pub struct HardwareRgb;

impl Module for HardwareRgb {
    fn name(&self) -> &'static str {
        "hardware.rgb"
    }

    fn config_key(&self) -> Option<&'static str> {
        Some("hardware_rgb")
    }

    fn description(&self) -> &'static str {
        "i2c-dev kernel module prerequisite for motherboard/SMBus RGB control"
    }

    fn discover(&self, _ctx: &Context) -> anyhow::Result<Observation> {
        let openrgb_installed = exec::command_available("openrgb");
        let i2c_related_modules = read_i2c_related_modules();
        let i2c_device_nodes = read_i2c_device_nodes();

        let data = RgbObservation {
            openrgb_installed,
            sdk_server_reachable: sdk_server_reachable(),
            i2c_dev_loaded: Path::new("/sys/module/i2c_dev").exists(),
            i2c_device_node_writable: i2c_device_nodes
                .first()
                .map(|path| can_open_read_write(path)),
            i2c_related_modules,
            i2c_device_nodes,
            modules_load_conf_current: fs::read_to_string(MODULES_LOAD_CONF_PATH).ok(),
            udev_rule_present: openrgb_udev_rule_present(),
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        if !data.i2c_dev_loaded && !data.i2c_related_modules.is_empty() {
            observation = observation.with_warning(format!(
                "an i2c module is loaded ({}) but `i2c-dev` is not, so no /dev/i2c-* device nodes exist",
                data.i2c_related_modules.join(", ")
            ));
        }
        if data.openrgb_installed && !data.udev_rule_present {
            observation = observation.with_warning(
                "openrgb is installed but no udev rule granting non-root device access was found under /etc/udev/rules.d (or the vendor equivalent) — USB-connected RGB controllers may only be controllable as root"
                    .to_string(),
            );
        }
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.hardware_rgb.enabled {
            return Diagnosis::compliant();
        }

        let data: RgbObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read hardware.rgb observation: {err}"
                )]);
            }
        };

        let mut findings = Vec::new();
        if !data.i2c_dev_loaded {
            findings.push(
                "`i2c-dev` kernel module is not loaded — /dev/i2c-* device nodes (needed for motherboard/SMBus RGB control) do not exist"
                    .to_string(),
            );
        }
        if data.modules_load_conf_current.as_deref() != Some(desired_modules_load_conf().as_str()) {
            findings.push(format!(
                "{MODULES_LOAD_CONF_PATH} does not declare `i2c-dev` to load at boot"
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
        if !ctx.config.hardware_rgb.enabled || diagnosis.compliant {
            return Ok(plan);
        }

        let data: RgbObservation = serde_json::from_value(observation.data.clone())?;
        let desired = desired_modules_load_conf();

        if data.modules_load_conf_current.as_deref() != Some(desired.as_str()) {
            plan.push(
                "declare `i2c-dev` to load at boot",
                Risk::Low,
                Change::WriteFile {
                    path: MODULES_LOAD_CONF_PATH.into(),
                    content: desired,
                },
            );
        }
        if !data.i2c_dev_loaded {
            plan.push(
                "load the `i2c-dev` kernel module now",
                Risk::Low,
                Change::RunCommand {
                    program: "modprobe".to_string(),
                    args: vec!["i2c-dev".to_string()],
                    privileged: true,
                },
            );
        }

        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        if !ctx.config.hardware_rgb.enabled {
            return Ok(vec![VerificationResult::skipped(
                "hardware.rgb",
                "disabled in config",
            )]);
        }

        let mut checks = Vec::new();
        let i2c_dev_loaded = Path::new("/sys/module/i2c_dev").exists();
        if i2c_dev_loaded {
            checks.push(VerificationResult::pass("i2c-dev kernel module is loaded"));
        } else {
            checks.push(VerificationResult::fail(
                "i2c-dev kernel module is loaded",
                "/sys/module/i2c_dev does not exist",
            ));
        }

        let nodes = read_i2c_device_nodes();
        if !i2c_dev_loaded {
            checks.push(VerificationResult::skipped(
                "/dev/i2c-* device nodes exist",
                "i2c-dev is not loaded",
            ));
        } else if nodes.is_empty() {
            checks.push(VerificationResult::fail(
                "/dev/i2c-* device nodes exist",
                "none found — check that an i2c adapter module (e.g. i2c-piix4) is also loaded",
            ));
        } else {
            checks.push(VerificationResult::pass("/dev/i2c-* device nodes exist"));
        }

        match nodes.first().map(|path| can_open_read_write(path)) {
            Some(true) => checks.push(VerificationResult::pass(
                "i2c device nodes are accessible without root",
            )),
            Some(false) => checks.push(VerificationResult::fail(
                "i2c device nodes are accessible without root",
                "could not open the device node for read+write as the invoking user — check group ownership/udev rules",
            )),
            None => checks.push(VerificationResult::skipped(
                "i2c device nodes are accessible without root",
                "no i2c device nodes exist to test",
            )),
        }

        Ok(checks)
    }
}

fn desired_modules_load_conf() -> String {
    "# Managed by DebKit.\ni2c-dev\n".to_string()
}

fn read_i2c_related_modules() -> Vec<String> {
    let Ok(entries) = fs::read_dir("/sys/module") else {
        return Vec::new();
    };
    let mut modules: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.starts_with("i2c"))
        .collect();
    modules.sort();
    modules
}

fn read_i2c_device_nodes() -> Vec<String> {
    let Ok(entries) = fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut nodes: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.starts_with("i2c-"))
        .map(|name| format!("/dev/{name}"))
        .collect();
    nodes.sort();
    nodes
}

fn can_open_read_write(path: &str) -> bool {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .is_ok()
}

fn sdk_server_reachable() -> bool {
    OPENRGB_SDK_ADDR
        .parse()
        .ok()
        .is_some_and(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok())
}

fn openrgb_udev_rule_present() -> bool {
    UDEV_RULE_DIRS.iter().any(|dir| {
        fs::read_dir(dir).is_ok_and(|entries| {
            entries.filter_map(|entry| entry.ok()).any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.to_lowercase().contains("openrgb"))
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(HardwareRgb.name(), "hardware.rgb");
    }

    #[test]
    fn desired_conf_declares_i2c_dev() {
        assert_eq!(
            desired_modules_load_conf(),
            "# Managed by DebKit.\ni2c-dev\n"
        );
    }

    #[test]
    fn sdk_server_unreachable_by_default_in_test_env() {
        // No OpenRGB SDK server is running in CI/dev sandboxes — exercises the real
        // connect-timeout path without depending on external state.
        assert!(!sdk_server_reachable());
    }

    fn observation(data: RgbObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn base_data() -> RgbObservation {
        RgbObservation {
            openrgb_installed: false,
            sdk_server_reachable: false,
            i2c_dev_loaded: true,
            i2c_related_modules: vec!["i2c_dev".to_string(), "i2c_piix4".to_string()],
            i2c_device_nodes: vec!["/dev/i2c-0".to_string()],
            i2c_device_node_writable: Some(true),
            modules_load_conf_current: Some(desired_modules_load_conf()),
            udev_rule_present: false,
        }
    }

    fn config(enabled: bool) -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.hardware_rgb.enabled = enabled;
        config
    }

    #[test]
    fn compliant_when_i2c_dev_loaded_and_declared() {
        let config = config(true);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareRgb.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
        let plan = HardwareRgb
            .plan(&ctx, &observation(base_data()), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn mismatch_when_i2c_dev_not_loaded_produces_write_and_modprobe() {
        let mut data = base_data();
        data.i2c_dev_loaded = false;
        data.i2c_related_modules = vec!["i2c_piix4".to_string()];
        data.i2c_device_nodes = Vec::new();
        data.i2c_device_node_writable = None;
        data.modules_load_conf_current = None;
        let config = config(true);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareRgb.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        assert_eq!(diagnosis.findings.len(), 2);
        let plan = HardwareRgb
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 2);
        assert!(matches!(plan.changes[0].change, Change::WriteFile { .. }));
        match &plan.changes[1].change {
            Change::RunCommand {
                program,
                args,
                privileged,
            } => {
                assert_eq!(program, "modprobe");
                assert_eq!(args, &["i2c-dev".to_string()]);
                assert!(privileged);
            }
            other => panic!("expected RunCommand, got {other:?}"),
        }
    }

    #[test]
    fn mismatch_when_loaded_but_not_persisted_only_writes_conf() {
        let mut data = base_data();
        data.modules_load_conf_current = None;
        let config = config(true);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareRgb.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = HardwareRgb
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 1);
        assert!(matches!(plan.changes[0].change, Change::WriteFile { .. }));
    }

    #[test]
    fn disabled_config_is_always_compliant() {
        let mut data = base_data();
        data.i2c_dev_loaded = false;
        data.modules_load_conf_current = None;
        let config = config(false);
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareRgb.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }
}
