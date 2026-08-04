/// One candidate owner for a resource (e.g. a DNS resolver, a DHCP server, a network
/// manager) and whether discovery found it currently active.
#[derive(Debug, Clone)]
pub struct OwnerProbe {
    pub name: String,
    pub active: bool,
}

impl OwnerProbe {
    pub fn new(name: impl Into<String>, active: bool) -> Self {
        Self {
            name: name.into(),
            active,
        }
    }
}

/// The result of resolving which owner(s) are currently active for a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerResult {
    /// Exactly one candidate is active: the resource has a single, unambiguous owner.
    Compliant(String),
    /// No candidate is active.
    NoOwner,
    /// More than one candidate is active at once. Modules must refuse to `plan()`
    /// against a resource in this state unless the user has set an explicit override.
    Conflict(Vec<String>),
}

impl OwnerResult {
    pub fn owner(&self) -> Option<&str> {
        match self {
            Self::Compliant(owner) => Some(owner.as_str()),
            Self::NoOwner | Self::Conflict(_) => None,
        }
    }

    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Conflict(_))
    }
}

/// Resolves ownership from a set of probes. `override_owner`, when set, short-circuits a
/// detected conflict as long as the override names one of the active candidates — this is
/// the generic form of "the user explicitly chose which owner wins."
pub fn detect_owner(probes: &[OwnerProbe], override_owner: Option<&str>) -> OwnerResult {
    let active: Vec<String> = probes
        .iter()
        .filter(|probe| probe.active)
        .map(|probe| probe.name.clone())
        .collect();

    if let Some(chosen) = override_owner
        && active.iter().any(|name| name == chosen)
    {
        return OwnerResult::Compliant(chosen.to_string());
    }

    match active.len() {
        0 => OwnerResult::NoOwner,
        1 => OwnerResult::Compliant(active.into_iter().next().expect("len == 1")),
        _ => OwnerResult::Conflict(active),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_active_probe_is_compliant() {
        let probes = vec![
            OwnerProbe::new("dnsmasq", true),
            OwnerProbe::new("systemd-resolved", false),
        ];
        assert_eq!(
            detect_owner(&probes, None),
            OwnerResult::Compliant("dnsmasq".to_string())
        );
    }

    #[test]
    fn no_active_probes_is_no_owner() {
        let probes = vec![OwnerProbe::new("dnsmasq", false)];
        assert_eq!(detect_owner(&probes, None), OwnerResult::NoOwner);
    }

    #[test]
    fn multiple_active_probes_conflict() {
        let probes = vec![
            OwnerProbe::new("dnsmasq", true),
            OwnerProbe::new("systemd-resolved", true),
        ];
        assert_eq!(
            detect_owner(&probes, None),
            OwnerResult::Conflict(vec!["dnsmasq".to_string(), "systemd-resolved".to_string()])
        );
    }

    #[test]
    fn override_resolves_conflict_when_it_names_an_active_candidate() {
        let probes = vec![
            OwnerProbe::new("dnsmasq", true),
            OwnerProbe::new("systemd-resolved", true),
        ];
        assert_eq!(
            detect_owner(&probes, Some("systemd-resolved")),
            OwnerResult::Compliant("systemd-resolved".to_string())
        );
    }

    #[test]
    fn override_naming_an_inactive_candidate_does_not_resolve_conflict() {
        let probes = vec![
            OwnerProbe::new("dnsmasq", true),
            OwnerProbe::new("systemd-resolved", true),
        ];
        assert_eq!(
            detect_owner(&probes, Some("NetworkManager")),
            OwnerResult::Conflict(vec!["dnsmasq".to_string(), "systemd-resolved".to_string()])
        );
    }
}
