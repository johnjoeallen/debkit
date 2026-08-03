use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::ChangePlan;

/// Read-only diagnostics: failed units, timers, and overall system state. There is no
/// declared intent to enforce here (no `systemd` config section), so `plan()` never
/// produces changes — this module only ever reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FailedUnit {
    unit: String,
    load: String,
    active: String,
    sub: String,
    description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SystemdData {
    system_state: String,
    failed_units: Vec<FailedUnit>,
    /// Raw `systemctl list-timers --all` lines. Per-column timing fields (NEXT/LEFT/
    /// LAST/PASSED) have variable-width, space-containing formats that aren't worth a
    /// brittle parser for a diagnostics dump — the unit name is still the whitespace
    /// second-to-last token on each line, which is what `timer_units` extracts.
    timers_raw: Vec<String>,
    timer_units: Vec<String>,
}

pub struct SystemdUnits;

impl Module for SystemdUnits {
    fn name(&self) -> &'static str {
        "systemd.units"
    }

    fn discover(&self, _ctx: &Context) -> anyhow::Result<Observation> {
        let system_state = exec::capture("systemctl", &["is-system-running"])
            .unwrap_or_else(|err| err.to_string())
            .trim()
            .to_string();

        let failed_units = read_failed_units();
        let timers_raw = read_timer_lines();
        let timer_units = timers_raw
            .iter()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                fields
                    .len()
                    .checked_sub(2)
                    .and_then(|idx| fields.get(idx))
                    .map(|s| s.to_string())
            })
            .collect();

        let data = SystemdData {
            system_state,
            failed_units,
            timers_raw,
            timer_units,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        if !data.failed_units.is_empty() {
            observation = observation.with_warning(format!(
                "{} unit(s) failed: {}",
                data.failed_units.len(),
                data.failed_units
                    .iter()
                    .map(|unit| unit.unit.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(observation)
    }

    fn diagnose(&self, _ctx: &Context, observation: &Observation) -> Diagnosis {
        let data: SystemdData = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read systemd.units observation: {err}"
                )]);
            }
        };

        if data.failed_units.is_empty() {
            Diagnosis::compliant()
        } else {
            Diagnosis::mismatch(
                data.failed_units
                    .iter()
                    .map(|unit| {
                        format!(
                            "{} is failed (load={}, active={}, sub={}): {}",
                            unit.unit, unit.load, unit.active, unit.sub, unit.description
                        )
                    })
                    .collect(),
            )
        }
    }

    fn plan(
        &self,
        _ctx: &Context,
        _observation: &Observation,
        _diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        // Diagnostics only: DebKit does not decide how to fix an arbitrary failed unit.
        Ok(ChangePlan::new())
    }

    fn verify(&self, _ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let failed = read_failed_units();
        if failed.is_empty() {
            Ok(vec![VerificationResult::pass("no failed systemd units")])
        } else {
            Ok(vec![VerificationResult::fail(
                "no failed systemd units",
                format!(
                    "{} unit(s) failed: {}",
                    failed.len(),
                    failed
                        .iter()
                        .map(|unit| unit.unit.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )])
        }
    }
}

fn read_failed_units() -> Vec<FailedUnit> {
    let Ok(raw) = exec::capture(
        "systemctl",
        &["--failed", "--no-legend", "--plain", "--all"],
    ) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 4 {
                return None;
            }
            Some(FailedUnit {
                unit: fields[0].to_string(),
                load: fields[1].to_string(),
                active: fields[2].to_string(),
                sub: fields[3].to_string(),
                description: fields[4..].join(" "),
            })
        })
        .collect()
}

fn read_timer_lines() -> Vec<String> {
    let Ok(raw) = exec::capture("systemctl", &["list-timers", "--all", "--no-legend"]) else {
        return Vec::new();
    };
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(SystemdUnits.name(), "systemd.units");
    }

    fn config() -> crate::config::DebkitConfig {
        crate::config::DebkitConfig::default()
    }

    #[test]
    fn compliant_when_no_failed_units() {
        let data = SystemdData {
            system_state: "running".to_string(),
            failed_units: Vec::new(),
            timers_raw: Vec::new(),
            timer_units: Vec::new(),
        };
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = SystemdUnits.diagnose(&ctx, &observation);
        assert!(diagnosis.compliant);
        let plan = SystemdUnits.plan(&ctx, &observation, &diagnosis).unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn mismatch_when_units_failed() {
        let data = SystemdData {
            system_state: "degraded".to_string(),
            failed_units: vec![FailedUnit {
                unit: "foo.service".to_string(),
                load: "loaded".to_string(),
                active: "failed".to_string(),
                sub: "failed".to_string(),
                description: "Foo service".to_string(),
            }],
            timers_raw: Vec::new(),
            timer_units: Vec::new(),
        };
        let config = config();
        let ctx = Context {
            hostname: "tornado".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = SystemdUnits.diagnose(&ctx, &observation);
        assert!(!diagnosis.compliant);
        assert!(diagnosis.findings[0].contains("foo.service"));
        // Diagnostics-only module: even a mismatch never produces a plan.
        let plan = SystemdUnits.plan(&ctx, &observation, &diagnosis).unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn extracts_failed_unit_fields() {
        let raw_line = "foo.service loaded failed failed Foo service description here";
        let fields: Vec<&str> = raw_line.split_whitespace().collect();
        let unit = FailedUnit {
            unit: fields[0].to_string(),
            load: fields[1].to_string(),
            active: fields[2].to_string(),
            sub: fields[3].to_string(),
            description: fields[4..].join(" "),
        };
        assert_eq!(unit.unit, "foo.service");
        assert_eq!(unit.description, "Foo service description here");
    }
}
