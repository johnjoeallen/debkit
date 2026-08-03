pub mod core_inspect;
pub mod network_wake_on_lan;

use crate::engine::module::Module;

/// Every module wired into the `inspect`/`diagnose`/`plan`/`apply`/`verify`/`status`
/// lifecycle commands. Toolchain installers (rust, npm, essentials, ...) stay under
/// `install/` and are not registered here — they don't need ownership/plan machinery.
pub fn registry() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(core_inspect::CoreInspect),
        Box::new(network_wake_on_lan::NetworkWakeOnLan),
    ]
}

pub fn find(name: &str) -> Option<Box<dyn Module>> {
    registry().into_iter().find(|module| module.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_includes_core_inspect() {
        assert!(
            registry()
                .iter()
                .any(|module| module.name() == "core.inspect")
        );
    }

    #[test]
    fn find_returns_none_for_unknown_module() {
        assert!(find("not.a.module").is_none());
    }

    #[test]
    fn find_returns_matching_module() {
        assert_eq!(find("core.inspect").unwrap().name(), "core.inspect");
    }
}
