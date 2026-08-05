//! Full documentation pages for `debkit info <name>`, embedded at compile time from the
//! same mdBook sources (`docs/src/`) that are published to GitHub Pages, so the CLI
//! shows identical content regardless of how/where the binary was installed (the
//! `docs/` tree itself isn't shipped in the .deb — see `Cargo.toml`'s `[package.metadata.deb]`
//! assets).

/// Returns the doc page for an install target or lifecycle module name, or `None` if
/// there isn't one.
pub fn doc_for(name: &str) -> Option<&'static str> {
    Some(match name {
        // Install targets (docs/src/installers/*.md, except where a target's docs
        // live in a standalone top-level page shared across several related targets).
        "essentials" => include_str!("../docs/src/installers/essentials.md"),
        "git" => include_str!("../docs/src/installers/git.md"),
        "git-prompt" => include_str!("../docs/src/installers/git-prompt.md"),
        "npm" => include_str!("../docs/src/installers/npm.md"),
        "nis" | "nis-client" | "nis-server" => include_str!("../docs/src/nis.md"),
        "claude" => include_str!("../docs/src/installers/claude.md"),
        "codex" => include_str!("../docs/src/installers/codex.md"),
        "ripgrep" => include_str!("../docs/src/installers/ripgrep.md"),
        "rust" => include_str!("../docs/src/installers/rust.md"),
        "sudo-nopass" => include_str!("../docs/src/sudo.md"),
        "variety" => include_str!("../docs/src/installers/variety.md"),
        "foundation" => include_str!("../docs/src/installers/foundation.md"),
        "wake-on-lan" => include_str!("../docs/src/wake-on-lan.md"),

        // Lifecycle modules (docs/src/modules/*.md).
        "core.inspect" => include_str!("../docs/src/modules/core-inspect.md"),
        "network.interfaces" => include_str!("../docs/src/modules/network-interfaces.md"),
        "network.dhcp" => include_str!("../docs/src/modules/network-dhcp.md"),
        "network.dns" => include_str!("../docs/src/modules/network-dns.md"),
        "network.firewall" => include_str!("../docs/src/modules/network-firewall.md"),
        "network.tailscale" => include_str!("../docs/src/modules/network-tailscale.md"),
        "network.wake_on_lan" => include_str!("../docs/src/modules/network-wake-on-lan.md"),
        "identity.nis" => include_str!("../docs/src/modules/identity-nis.md"),
        "identity.nss" => include_str!("../docs/src/modules/identity-nss.md"),
        "identity.pam" => include_str!("../docs/src/modules/identity-pam.md"),
        "identity.sudo" => include_str!("../docs/src/modules/identity-sudo.md"),
        "systemd.units" => include_str!("../docs/src/modules/systemd-units.md"),
        "developer.git" => include_str!("../docs/src/modules/developer-git.md"),
        "apt.repositories" => include_str!("../docs/src/modules/apt-repositories.md"),
        "boot.grub" => include_str!("../docs/src/modules/boot-grub.md"),
        "hardware.sleep" => include_str!("../docs/src/modules/hardware-sleep.md"),
        "hardware.rgb" => include_str!("../docs/src/modules/hardware-rgb.md"),

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_install_target_doc() {
        assert!(doc_for("claude").unwrap().contains("Claude Code CLI"));
    }

    #[test]
    fn resolves_shared_nis_doc_for_every_nis_target() {
        let nis = doc_for("nis").unwrap();
        assert_eq!(doc_for("nis-client").unwrap(), nis);
        assert_eq!(doc_for("nis-server").unwrap(), nis);
    }

    #[test]
    fn resolves_module_doc() {
        assert!(doc_for("boot.grub").unwrap().contains("GRUB"));
    }

    #[test]
    fn returns_none_for_unknown_name() {
        assert!(doc_for("not-a-real-target-or-module").is_none());
    }

    #[test]
    fn every_install_target_has_a_doc() {
        for target in crate::install::targets() {
            assert!(
                doc_for(target.name).is_some(),
                "missing `debkit info` doc for install target `{}`",
                target.name
            );
        }
    }

    #[test]
    fn every_module_has_a_doc() {
        for module in crate::modules::registry() {
            assert!(
                doc_for(module.name()).is_some(),
                "missing `debkit info` doc for module `{}`",
                module.name()
            );
        }
    }
}
