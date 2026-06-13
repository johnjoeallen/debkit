use anyhow::Context;

use crate::config::DebkitConfig;

pub fn run(config: &DebkitConfig) -> anyhow::Result<()> {
    if config.foundation.install.is_empty() {
        println!("No foundation install targets configured (`foundation.install` is empty).");
        return Ok(());
    }

    for target in &config.foundation.install {
        match target.as_str() {
            "codex" => {
                println!("Installing foundation target: codex");
                super::codex::run(config.npm.version.clone())
                    .context("failed to install foundation target `codex`")?;
            }
            "git" => {
                println!("Installing foundation target: git");
                super::git::run().context("failed to install foundation target `git`")?;
            }
            "npm" => {
                println!("Installing foundation target: npm");
                super::npm::run(super::npm::Options {
                    version: config.npm.version.clone(),
                })
                .context("failed to install foundation target `npm`")?;
            }
            "sudo-nopass" | "sudo_nopass" | "admin-group-nopass" | "admin_group_nopass" => {
                println!("Installing foundation target: sudo-nopass");
                super::sudo_nopass::run(&config.sudo_nopass)
                    .context("failed to install foundation target `sudo-nopass`")?;
            }
            "nis" => {
                println!("Installing foundation target: nis");
                super::nis::run(super::nis::Role::Configured, &config.nis)
                    .context("failed to install foundation target `nis`")?;
            }
            "nis-client" | "nis_client" => {
                println!("Installing foundation target: nis-client");
                super::nis::run(super::nis::Role::Client, &config.nis)
                    .context("failed to install foundation target `nis-client`")?;
            }
            "nis-server" | "nis_server" => {
                println!("Installing foundation target: nis-server");
                super::nis::run(super::nis::Role::Server, &config.nis)
                    .context("failed to install foundation target `nis-server`")?;
            }
            "ripgrep" => {
                println!("Installing foundation target: ripgrep");
                super::ripgrep::run().context("failed to install foundation target `ripgrep`")?;
            }
            "rust" => {
                println!("Installing foundation target: rust");
                super::rust::run(super::rust::Options { reinstall: false })
                    .context("failed to install foundation target `rust`")?;
            }
            "variety" => {
                println!("Installing foundation target: variety");
                super::variety::run(config)
                    .context("failed to install foundation target `variety`")?;
            }
            "wake-on-lan" | "wake_on_lan" | "wol" => {
                println!("Installing foundation target: wake-on-lan");
                super::wake_on_lan::run(config)
                    .context("failed to install foundation target `wake-on-lan`")?;
            }
            other => {
                eprintln!("warning: unsupported foundation target `{other}` in config; skipping");
            }
        }
    }

    Ok(())
}
