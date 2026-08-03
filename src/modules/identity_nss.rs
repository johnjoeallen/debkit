use std::collections::HashMap;
use std::fs;

use serde::{Deserialize, Serialize};

use crate::engine::evidence::VerificationResult;
use crate::engine::exec;
use crate::engine::module::{Context, Diagnosis, Module, Observation};
use crate::engine::plan::ChangePlan;

/// Read-only: local vs. NIS identity collisions, and whether local recovery access
/// (`root`) still resolves without NIS in the loop. `nsswitch.conf` *content* is already
/// owned by `identity.nis`'s plan()/apply() — this module's distinct job is the
/// diagnostic doc §6.3 calls for that isn't just "is the file correct": comparing local
/// `/etc/passwd`/`/etc/group` against what NIS actually serves. There's nothing here to
/// declare or apply; a UID/GID collision isn't something DebKit should silently resolve.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdCollision {
    id: u32,
    local_name: String,
    nis_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NssObservation {
    nis_enabled: bool,
    nis_bound: bool,
    root_resolves_locally: bool,
    uid_collisions: Vec<IdCollision>,
    gid_collisions: Vec<IdCollision>,
}

pub struct IdentityNss;

impl Module for IdentityNss {
    fn name(&self) -> &'static str {
        "identity.nss"
    }

    fn discover(&self, ctx: &Context) -> anyhow::Result<Observation> {
        let nis_enabled = ctx.config.nis.enabled;
        let root_resolves_locally =
            exec::capture("getent", &["-s", "files", "passwd", "root"]).is_ok();

        let (nis_bound, uid_collisions, gid_collisions) = if nis_enabled {
            let local_passwd = parse_local_ids("/etc/passwd");
            let local_group = parse_local_ids("/etc/group");
            let nis_passwd = ypcat_ids("passwd.byname");
            let nis_group = ypcat_ids("group.byname");
            let bound = nis_passwd.is_some() || nis_group.is_some();
            (
                bound,
                find_collisions(&local_passwd, nis_passwd.as_ref()),
                find_collisions(&local_group, nis_group.as_ref()),
            )
        } else {
            (false, Vec::new(), Vec::new())
        };

        let data = NssObservation {
            nis_enabled,
            nis_bound,
            root_resolves_locally,
            uid_collisions,
            gid_collisions,
        };

        let mut observation = Observation::new(serde_json::to_value(&data)?);
        if !data.root_resolves_locally {
            observation = observation.with_warning(
                "`getent -s files passwd root` failed — root does not resolve without NIS in the loop",
            );
        }
        if data.nis_enabled && !data.nis_bound {
            observation = observation.with_warning(
                "NIS is enabled but `ypcat` returned nothing; is this host bound yet?",
            );
        }
        Ok(observation)
    }

    fn diagnose(&self, _ctx: &Context, observation: &Observation) -> Diagnosis {
        let data: NssObservation = match serde_json::from_value(observation.data.clone()) {
            Ok(data) => data,
            Err(err) => {
                return Diagnosis::mismatch(vec![format!(
                    "failed to read identity.nss observation: {err}"
                )]);
            }
        };

        let mut findings = Vec::new();
        if !data.root_resolves_locally {
            findings.push(
                "root does not resolve via `files` alone — local recovery access is at risk if NIS is down"
                    .to_string(),
            );
        }
        for collision in &data.uid_collisions {
            findings.push(format!(
                "UID {} is used locally by `{}` and by NIS user `{}`",
                collision.id, collision.local_name, collision.nis_name
            ));
        }
        for collision in &data.gid_collisions {
            findings.push(format!(
                "GID {} is used locally by `{}` and by NIS group `{}`",
                collision.id, collision.local_name, collision.nis_name
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
        _ctx: &Context,
        _observation: &Observation,
        _diagnosis: &Diagnosis,
    ) -> anyhow::Result<ChangePlan> {
        // Diagnostics only — see module doc comment.
        Ok(ChangePlan::new())
    }

    fn verify(&self, _ctx: &Context) -> anyhow::Result<Vec<VerificationResult>> {
        let mut checks = Vec::new();
        checks.push(
            if exec::capture("getent", &["-s", "files", "passwd", "root"]).is_ok() {
                VerificationResult::pass("root resolves via `files` without NIS")
            } else {
                VerificationResult::fail(
                    "root resolves via `files` without NIS",
                    "`getent -s files passwd root` failed",
                )
            },
        );
        Ok(checks)
    }
}

fn parse_local_ids(path: &str) -> HashMap<u32, String> {
    let Ok(raw) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    raw.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let id = fields.nth(1)?.parse::<u32>().ok()?;
            Some((id, name.to_string()))
        })
        .collect()
}

/// `ypcat <map>` prints `name:...:id:...`, matching the same colon-delimited layout as
/// `/etc/passwd`/`/etc/group`, so the parser is shared. Returns `None` (not empty) when
/// `ypcat` fails, so callers can distinguish "not bound to NIS" from "bound, no entries".
fn ypcat_ids(map: &str) -> Option<HashMap<u32, String>> {
    let raw = exec::capture("ypcat", &[map]).ok()?;
    Some(
        raw.lines()
            .filter_map(|line| {
                let mut fields = line.split(':');
                let name = fields.next()?;
                let id = fields.nth(1)?.parse::<u32>().ok()?;
                Some((id, name.to_string()))
            })
            .collect(),
    )
}

fn find_collisions(
    local: &HashMap<u32, String>,
    nis: Option<&HashMap<u32, String>>,
) -> Vec<IdCollision> {
    let Some(nis) = nis else {
        return Vec::new();
    };
    let mut collisions: Vec<IdCollision> = local
        .iter()
        .filter_map(|(id, local_name)| {
            nis.get(id).and_then(|nis_name| {
                (nis_name != local_name).then(|| IdCollision {
                    id: *id,
                    local_name: local_name.clone(),
                    nis_name: nis_name.clone(),
                })
            })
        })
        .collect();
    collisions.sort_by_key(|collision| collision.id);
    collisions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_name_matches_config_section() {
        assert_eq!(IdentityNss.name(), "identity.nss");
    }

    #[test]
    fn same_name_at_same_id_is_not_a_collision() {
        let mut local = HashMap::new();
        local.insert(1010, "jallen".to_string());
        let mut nis = HashMap::new();
        nis.insert(1010, "jallen".to_string());
        assert!(find_collisions(&local, Some(&nis)).is_empty());
    }

    #[test]
    fn different_name_at_same_id_is_a_collision() {
        let mut local = HashMap::new();
        local.insert(1010, "localuser".to_string());
        let mut nis = HashMap::new();
        nis.insert(1010, "jallen".to_string());
        let collisions = find_collisions(&local, Some(&nis));
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].local_name, "localuser");
        assert_eq!(collisions[0].nis_name, "jallen");
    }

    #[test]
    fn unbound_nis_produces_no_collisions() {
        let mut local = HashMap::new();
        local.insert(1010, "localuser".to_string());
        assert!(find_collisions(&local, None).is_empty());
    }

    #[test]
    fn parses_colon_delimited_id_lines() {
        let raw = "jallen:x:1010:1010:John Allen:/home/jallen:/bin/bash";
        let mut fields = raw.split(':');
        let name = fields.next().unwrap();
        let id: u32 = fields.nth(1).unwrap().parse().unwrap();
        assert_eq!(name, "jallen");
        assert_eq!(id, 1010);
    }

    #[test]
    fn compliant_when_no_findings() {
        let data = NssObservation {
            nis_enabled: true,
            nis_bound: true,
            root_resolves_locally: true,
            uid_collisions: Vec::new(),
            gid_collisions: Vec::new(),
        };
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = IdentityNss.diagnose(&ctx, &observation);
        assert!(diagnosis.compliant);
    }

    #[test]
    fn mismatch_when_root_does_not_resolve_locally() {
        let data = NssObservation {
            nis_enabled: false,
            nis_bound: false,
            root_resolves_locally: false,
            uid_collisions: Vec::new(),
            gid_collisions: Vec::new(),
        };
        let config = crate::config::DebkitConfig::default();
        let ctx = Context {
            hostname: "iris".to_string(),
            config: &config,
        };
        let observation = Observation::new(serde_json::to_value(&data).unwrap());
        let diagnosis = IdentityNss.diagnose(&ctx, &observation);
        assert!(!diagnosis.compliant);
    }
}
