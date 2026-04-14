pub mod codex;
pub mod foundation;
pub mod list;
pub mod npm;
pub mod ripgrep;
pub mod rust;
pub mod variety;

#[derive(Debug, Clone, Copy)]
pub struct InstallTarget {
    pub name: &'static str,
    pub supports_install: bool,
    pub supports_configure: bool,
    pub description: &'static str,
}

pub fn targets() -> &'static [InstallTarget] {
    &[
        InstallTarget {
            name: "npm",
            supports_install: true,
            supports_configure: false,
            description: "Node.js and npm from official Node.js binaries",
        },
        InstallTarget {
            name: "codex",
            supports_install: true,
            supports_configure: false,
            description: "OpenAI Codex CLI via npm",
        },
        InstallTarget {
            name: "ripgrep",
            supports_install: true,
            supports_configure: false,
            description: "ripgrep recursive search tool",
        },
        InstallTarget {
            name: "rust",
            supports_install: true,
            supports_configure: true,
            description: "Rust toolchain via rustup",
        },
        InstallTarget {
            name: "variety",
            supports_install: true,
            supports_configure: true,
            description: "Variety wallpaper rotator for GNOME",
        },
        InstallTarget {
            name: "foundation",
            supports_install: true,
            supports_configure: false,
            description: "Installs configured base targets from debkit config",
        },
    ]
}
