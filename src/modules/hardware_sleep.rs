use std::fs;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::{Change, ChangePlan, Risk};

const LOGIND_DEST: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_IFACE: &str = "org.freedesktop.login1.Manager";

/// One inhibitor lock currently held via logind, filtered (in `read_sleep_inhibitors`)
/// to only `what` values that include `sleep`. Surfaced for troubleshooting context
/// only — see the module doc comment for why this deliberately isn't a `diagnose()`
/// finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SleepInhibitor {
    what: String,
    who: String,
    why: String,
    mode: String,
}

/// Suspend/resume-reliability diagnostics and one narrow, safe piece of declarative
/// state: the active `/sys/power/mem_sleep` mode. Scoped from the requirements doc's
/// AM5/desktop suspend troubleshooting: platforms that boot with the (often
/// power-hungry, sometimes unreliable-to-resume) `s2idle` mode active when `deep` (real
/// S3) is supported and preferred, plus spurious-wake sources and blocked suspends.
///
/// Everything here is read without root: `/sys/power/mem_sleep`, `/sys/power/state`,
/// and `/proc/acpi/wakeup` are world-readable, and `busctl get-property`/`call` against
/// `org.freedesktop.login1` are read-only D-Bus calls any user can make.
///
/// Deliberately NOT surfaced as `diagnose()` findings: enabled wakeup devices (a
/// keyboard, power button, or this host's own `network.wake_on_lan`-managed NIC being
/// an enabled wakeup source is normal, not a defect — there's no way to tell "expected"
/// from "spurious" without the user declaring which devices they expect, which this
/// version doesn't ask for) and sleep inhibitors (GNOME/NetworkManager/UPower hold
/// routine "delay"-mode locks on essentially every real desktop, and even "block"-mode
/// session-manager locks are normal steady state, released internally when
/// appropriate — flagging them would be false-positive noise on a healthy system, not
/// a real diagnostic signal). Both are still recorded in the Observation for `debkit
/// inspect`/troubleshooting visibility.
///
/// Actually triggering a suspend/resume cycle to verify it works is out of scope for
/// `verify()`: it would kill the very SSH session used to run `debkit`, and require a
/// physical wake afterward. `verify()` instead re-checks the two things this module can
/// assert without disrupting the session: platform-reported suspend-to-RAM capability,
/// and (if declared) that `mem_sleep` actually took the declared value.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HardwareSleepObservation {
    dbus_available: bool,
    mem_sleep_available: Vec<String>,
    mem_sleep_active: Option<String>,
    power_states_available: Vec<String>,
    suspend_to_ram_available: bool,
    hibernate_available: bool,
    wakeup_enabled_devices: Vec<String>,
    sleep_inhibitors: Vec<SleepInhibitor>,
    logind_handle_lid_switch: Option<String>,
    logind_handle_suspend_key: Option<String>,
    logind_idle_action: Option<String>,
    /// logind's own `CanSuspend` judgement: `"yes"`, `"no"`, `"challenge"` (requires
    /// polkit auth), or `"na"`. `"challenge"` is routine under a normal desktop polkit
    /// policy, not itself a problem.
    can_suspend: Option<String>,
}

pub struct HardwareSleep;

impl Module for HardwareSleep {
    fn name(&self) -> &'static str {
        "hardware.sleep"
    }

    fn config_key(&self) -> Option<&'static str> {
        Some("hardware_sleep")
    }

    fn description(&self) -> &'static str {
        "suspend/resume diagnostics and the active /sys/power/mem_sleep mode"
    }

    fn discover(&self, _ctx: &Context) -> anyhow::Result<Observation> {
        let (mem_sleep_available, mem_sleep_active) = read_mem_sleep();
        let power_states_available = read_power_states();
        let suspend_to_ram_available = power_states_available.iter().any(|s| s == "mem");
        let hibernate_available = power_states_available.iter().any(|s| s == "disk");
        let wakeup_enabled_devices = read_wakeup_enabled_devices();

        let dbus_available = exec::command_available("busctl");
        let sleep_inhibitors = if dbus_available {
            read_sleep_inhibitors()
        } else {
            Vec::new()
        };
        let logind_handle_lid_switch = dbus_available
            .then(|| busctl_get_property("HandleLidSwitch"))
            .flatten();
        let logind_handle_suspend_key = dbus_available
            .then(|| busctl_get_property("HandleSuspendKey"))
            .flatten();
        let logind_idle_action = dbus_available
            .then(|| busctl_get_property("IdleAction"))
            .flatten();
        let can_suspend = dbus_available.then(busctl_can_suspend).flatten();

        let data = HardwareSleepObservation {
            dbus_available,
            mem_sleep_available,
            mem_sleep_active,
            power_states_available,
            suspend_to_ram_available,
            hibernate_available,
            wakeup_enabled_devices,
            sleep_inhibitors,
            logind_handle_lid_switch,
            logind_handle_suspend_key,
            logind_idle_action,
            can_suspend,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        if !data.suspend_to_ram_available {
            observation = observation.with_warning(
                "platform does not report suspend-to-RAM support (`mem` not in /sys/power/state)"
                    .to_string(),
            );
        }
        if data.can_suspend.as_deref() == Some("no") {
            observation = observation.with_warning("logind reports CanSuspend=no".to_string());
        }
        Ok(observation)
    }

    fn diagnose(&self, ctx: &Context, observation: &Observation) -> Diagnosis {
        if !ctx.config.hardware_sleep.enabled {
            return Diagnosis::compliant();
        }

        let data: HardwareSleepObservation = match serde_json::from_value(observation.data.clone())
        {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read hardware.sleep observation: {err}"
                )]);
            }
        };

        let mut findings = Vec::new();

        let desired = ctx.config.hardware_sleep.desired_mem_sleep.trim();
        if !desired.is_empty() {
            if !data.mem_sleep_available.iter().any(|mode| mode == desired) {
                findings.push(format!(
                    "declared mem_sleep mode `{desired}` is not supported by this platform (available: {})",
                    data.mem_sleep_available.join(", ")
                ));
            } else if data.mem_sleep_active.as_deref() != Some(desired) {
                findings.push(format!(
                    "mem_sleep is `{}`, declared `{desired}`",
                    data.mem_sleep_active.as_deref().unwrap_or("unknown")
                ));
            }
        }

        if !data.suspend_to_ram_available {
            findings.push(
                "this platform does not report suspend-to-RAM support (`mem` not in /sys/power/state)"
                    .to_string(),
            );
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
        if !ctx.config.hardware_sleep.enabled || diagnosis.compliant {
            return Ok(plan);
        }

        let desired = ctx.config.hardware_sleep.desired_mem_sleep.trim();
        if desired.is_empty() {
            return Ok(plan);
        }

        let data: HardwareSleepObservation = serde_json::from_value(observation.data.clone())?;
        if !data.mem_sleep_available.iter().any(|mode| mode == desired) {
            // Flagged by diagnose() but not fixable: the platform doesn't offer this
            // mode at all, so there's nothing to write.
            return Ok(plan);
        }
        if data.mem_sleep_active.as_deref() != Some(desired) {
            plan.push(
                format!("set /sys/power/mem_sleep to `{desired}`"),
                Risk::Low,
                Change::RunCommand {
                    program: "sh".to_string(),
                    args: vec![
                        "-c".to_string(),
                        format!("echo {desired} > /sys/power/mem_sleep"),
                    ],
                    privileged: true,
                },
            );
        }

        Ok(plan)
    }

    fn verify(&self, ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        if !ctx.config.hardware_sleep.enabled {
            return Ok(vec![VerificationResult::skipped(
                "hardware.sleep",
                "disabled in config",
            )]);
        }

        let mut checks = Vec::new();

        let power_states = read_power_states();
        if power_states.iter().any(|s| s == "mem") {
            checks.push(VerificationResult::pass(
                "platform reports suspend-to-RAM support",
            ));
        } else {
            checks.push(VerificationResult::fail(
                "platform reports suspend-to-RAM support",
                "`mem` not present in /sys/power/state",
            ));
        }

        let desired = ctx.config.hardware_sleep.desired_mem_sleep.trim();
        if desired.is_empty() {
            checks.push(VerificationResult::skipped(
                "mem_sleep matches declared mode",
                "`desired_mem_sleep` not declared",
            ));
        } else {
            let (available, active) = read_mem_sleep();
            if !available.iter().any(|mode| mode == desired) {
                checks.push(VerificationResult::skipped(
                    "mem_sleep matches declared mode",
                    format!(
                        "`{desired}` is not supported by this platform (available: {})",
                        available.join(", ")
                    ),
                ));
            } else if active.as_deref() == Some(desired) {
                checks.push(VerificationResult::pass("mem_sleep matches declared mode"));
            } else {
                checks.push(VerificationResult::fail(
                    "mem_sleep matches declared mode",
                    format!(
                        "active mode is `{}`, expected `{desired}`",
                        active.as_deref().unwrap_or("unknown")
                    ),
                ));
            }
        }

        Ok(checks)
    }
}

fn parse_mem_sleep(raw: &str) -> (Vec<String>, Option<String>) {
    let mut available = Vec::new();
    let mut active = None;
    for token in raw.split_whitespace() {
        match token.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            Some(inner) => {
                active = Some(inner.to_string());
                available.push(inner.to_string());
            }
            None => available.push(token.to_string()),
        }
    }
    (available, active)
}

fn read_mem_sleep() -> (Vec<String>, Option<String>) {
    fs::read_to_string("/sys/power/mem_sleep")
        .ok()
        .map(|raw| parse_mem_sleep(raw.trim()))
        .unwrap_or_default()
}

fn read_power_states() -> Vec<String> {
    fs::read_to_string("/sys/power/state")
        .ok()
        .map(|raw| raw.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Parses `/proc/acpi/wakeup`'s `Device S-state Status [Sysfs node]` table, skipping
/// the header line. A leading `*` on the status column is a kernel-internal marker
/// unrelated to whether the device is currently armed as a wakeup source, so it's
/// stripped before comparing.
fn parse_acpi_wakeup(raw: &str) -> Vec<String> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = fields.next()?;
            let _sstate = fields.next()?;
            let status = fields.next()?.trim_start_matches('*');
            (status == "enabled").then(|| device.to_string())
        })
        .collect()
}

fn read_wakeup_enabled_devices() -> Vec<String> {
    fs::read_to_string("/proc/acpi/wakeup")
        .ok()
        .map(|raw| parse_acpi_wakeup(&raw))
        .unwrap_or_default()
}

/// `busctl --json=short get-property` wraps a single property value directly as
/// `data`; `busctl --json=short call` wraps each output parameter as an element of a
/// `data` array (even for a single-return-value method) — the two need different
/// unwrapping, handled by `busctl_get_property`/`busctl_can_suspend` respectively.
fn parse_busctl_property_string(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    value.get("data")?.as_str().map(str::to_string)
}

fn parse_busctl_call_string(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    value
        .get("data")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_string)
}

fn busctl_get_property(member: &str) -> Option<String> {
    let raw = exec::capture(
        "busctl",
        &[
            "--json=short",
            "get-property",
            LOGIND_DEST,
            LOGIND_PATH,
            LOGIND_IFACE,
            member,
        ],
    )
    .ok()?;
    parse_busctl_property_string(&raw)
}

fn busctl_can_suspend() -> Option<String> {
    let raw = exec::capture(
        "busctl",
        &[
            "--json=short",
            "call",
            LOGIND_DEST,
            LOGIND_PATH,
            LOGIND_IFACE,
            "CanSuspend",
        ],
    )
    .ok()?;
    parse_busctl_call_string(&raw)
}

fn parse_inhibitors_json(raw: &str) -> Vec<SleepInhibitor> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(entries) = value
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .and_then(|inner| inner.as_array())
    else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let fields = entry.as_array()?;
            Some(SleepInhibitor {
                what: fields.first()?.as_str()?.to_string(),
                who: fields.get(1)?.as_str()?.to_string(),
                why: fields.get(2)?.as_str()?.to_string(),
                mode: fields.get(3)?.as_str()?.to_string(),
            })
        })
        .filter(|inhibitor| inhibitor.what.split(':').any(|token| token == "sleep"))
        .collect()
}

fn read_sleep_inhibitors() -> Vec<SleepInhibitor> {
    let Ok(raw) = exec::capture(
        "busctl",
        &[
            "--json=short",
            "call",
            LOGIND_DEST,
            LOGIND_PATH,
            LOGIND_IFACE,
            "ListInhibitors",
        ],
    ) else {
        return Vec::new();
    };
    parse_inhibitors_json(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(HardwareSleep.name(), "hardware.sleep");
    }

    #[test]
    fn parses_mem_sleep_bracketed_active_mode() {
        let (available, active) = parse_mem_sleep("s2idle [deep]");
        assert_eq!(available, vec!["s2idle", "deep"]);
        assert_eq!(active.as_deref(), Some("deep"));
    }

    #[test]
    fn parses_mem_sleep_single_mode() {
        let (available, active) = parse_mem_sleep("[s2idle]");
        assert_eq!(available, vec!["s2idle"]);
        assert_eq!(active.as_deref(), Some("s2idle"));
    }

    #[test]
    fn parses_acpi_wakeup_skipping_header_and_disabled_devices() {
        let raw = "Device\tS-state\t  Status   Sysfs node\nGPP3\t  S4\t*disabled\nXHC0\t  S3\t*enabled\tpci0000:00\n";
        assert_eq!(parse_acpi_wakeup(raw), vec!["XHC0"]);
    }

    #[test]
    fn parses_busctl_get_property_reply() {
        let raw = r#"{"type":"s","data":"suspend"}"#;
        assert_eq!(
            parse_busctl_property_string(raw).as_deref(),
            Some("suspend")
        );
    }

    #[test]
    fn parses_busctl_call_reply() {
        let raw = r#"{"type":"s","data":["challenge"]}"#;
        assert_eq!(parse_busctl_call_string(raw).as_deref(), Some("challenge"));
    }

    #[test]
    fn parses_inhibitors_and_filters_to_sleep_related() {
        let raw = r#"{"type":"a(ssssuu)","data":[[["handle-power-key:handle-suspend-key:handle-hibernate-key","jallen","GNOME handling keypresses","block",1002,5893],["sleep","NetworkManager","NetworkManager needs to turn off networks","delay",0,1340]]]}"#;
        let inhibitors = parse_inhibitors_json(raw);
        assert_eq!(inhibitors.len(), 1);
        assert_eq!(inhibitors[0].who, "NetworkManager");
        assert_eq!(inhibitors[0].mode, "delay");
    }

    fn observation(data: HardwareSleepObservation) -> Observation {
        Observation::new(serde_json::to_value(&data).unwrap())
    }

    fn base_data() -> HardwareSleepObservation {
        HardwareSleepObservation {
            dbus_available: true,
            mem_sleep_available: vec!["s2idle".to_string(), "deep".to_string()],
            mem_sleep_active: Some("deep".to_string()),
            power_states_available: vec!["freeze".to_string(), "mem".to_string()],
            suspend_to_ram_available: true,
            hibernate_available: false,
            wakeup_enabled_devices: Vec::new(),
            sleep_inhibitors: Vec::new(),
            logind_handle_lid_switch: Some("suspend".to_string()),
            logind_handle_suspend_key: Some("suspend".to_string()),
            logind_idle_action: Some("suspend".to_string()),
            can_suspend: Some("challenge".to_string()),
        }
    }

    fn config(desired_mem_sleep: &str) -> crate::config::DebkitConfig {
        let mut config = crate::config::DebkitConfig::default();
        config.hardware_sleep.enabled = true;
        config.hardware_sleep.desired_mem_sleep = desired_mem_sleep.to_string();
        config
    }

    #[test]
    fn compliant_when_active_mode_matches_desired() {
        let config = config("deep");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareSleep.diagnose(&ctx, &observation(base_data()));
        assert!(diagnosis.compliant);
        let plan = HardwareSleep
            .plan(&ctx, &observation(base_data()), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn mismatch_produces_sysfs_write_when_supported() {
        let mut data = base_data();
        data.mem_sleep_active = Some("s2idle".to_string());
        let config = config("deep");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareSleep.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        let plan = HardwareSleep
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert_eq!(plan.changes.len(), 1);
        match &plan.changes[0].change {
            Change::RunCommand {
                program,
                args,
                privileged,
            } => {
                assert_eq!(program, "sh");
                assert!(args[1].contains("echo deep > /sys/power/mem_sleep"));
                assert!(privileged);
            }
            other => panic!("expected RunCommand, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_desired_mode_is_flagged_but_not_planned() {
        let mut data = base_data();
        data.mem_sleep_available = vec!["s2idle".to_string()];
        data.mem_sleep_active = Some("s2idle".to_string());
        let config = config("deep");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareSleep.diagnose(&ctx, &observation(data.clone()));
        assert!(!diagnosis.compliant);
        assert!(diagnosis.findings[0].contains("not supported by this platform"));
        let plan = HardwareSleep
            .plan(&ctx, &observation(data), &diagnosis)
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn no_suspend_support_is_flagged() {
        let mut data = base_data();
        data.suspend_to_ram_available = false;
        data.power_states_available = vec!["freeze".to_string()];
        let config = config("");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareSleep.diagnose(&ctx, &observation(data));
        assert!(!diagnosis.compliant);
        assert!(diagnosis.findings[0].contains("does not report suspend-to-RAM support"),);
    }

    #[test]
    fn empty_desired_mode_skips_the_mem_sleep_check() {
        let mut data = base_data();
        data.mem_sleep_active = Some("s2idle".to_string());
        let config = config("");
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareSleep.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }

    #[test]
    fn disabled_config_is_always_compliant() {
        let mut data = base_data();
        data.suspend_to_ram_available = false;
        let mut config = config("deep");
        config.hardware_sleep.enabled = false;
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let diagnosis = HardwareSleep.diagnose(&ctx, &observation(data));
        assert!(diagnosis.compliant);
    }
}
