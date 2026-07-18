pub mod apt;
pub mod codex;
pub mod essentials;
pub mod foundation;
pub mod git;
pub mod git_prompt;
pub mod list;
pub mod nis;
pub mod npm;
pub mod ripgrep;
pub mod rust;
pub mod sudo_nopass;
pub mod variety;
pub mod wake_on_lan;

#[derive(Debug, Clone, Copy)]
pub struct InstallTarget {
    pub name: &'static str,
    pub supports_install: bool,
    pub supports_uninstall: bool,
    pub supports_configure: bool,
    pub description: &'static str,
}

pub fn targets() -> &'static [InstallTarget] {
    &[
        InstallTarget {
            name: "essentials",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: false,
            description: "Baseline CLI packages required for provisioning",
        },
        InstallTarget {
            name: "git",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: false,
            description: "Git version control via apt",
        },
        InstallTarget {
            name: "git-prompt",
            supports_install: false,
            supports_uninstall: false,
            supports_configure: true,
            description: "Git-aware Bash prompt for the current user",
        },
        InstallTarget {
            name: "npm",
            supports_install: true,
            supports_uninstall: true,
            supports_configure: false,
            description: "Node.js and npm from official Node.js binaries",
        },
        InstallTarget {
            name: "nis",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: true,
            description: "NIS client and server packages",
        },
        InstallTarget {
            name: "nis-client",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: true,
            description: "NIS client packages",
        },
        InstallTarget {
            name: "nis-server",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: true,
            description: "NIS server packages",
        },
        InstallTarget {
            name: "codex",
            supports_install: true,
            supports_uninstall: true,
            supports_configure: false,
            description: "OpenAI Codex CLI via npm",
        },
        InstallTarget {
            name: "ripgrep",
            supports_install: true,
            supports_uninstall: true,
            supports_configure: false,
            description: "ripgrep recursive search tool",
        },
        InstallTarget {
            name: "rust",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: true,
            description: "Rust toolchain via rustup",
        },
        InstallTarget {
            name: "sudo-nopass",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: false,
            description: "Passwordless sudo for configured users",
        },
        InstallTarget {
            name: "variety",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: true,
            description: "Variety wallpaper rotator for GNOME",
        },
        InstallTarget {
            name: "foundation",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: false,
            description: "Installs configured base targets from debkit config",
        },
        InstallTarget {
            name: "wake-on-lan",
            supports_install: true,
            supports_uninstall: false,
            supports_configure: false,
            description: "Inspect and enable wired Ethernet Wake-on-LAN",
        },
    ]
}
