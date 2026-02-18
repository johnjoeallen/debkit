use anyhow::Context;

use crate::config::DebkitConfig;

pub fn run(config: &DebkitConfig) -> anyhow::Result<()> {
    if config.foundation.install.is_empty() {
        println!("No foundation install targets configured (`foundation.install` is empty).");
        return Ok(());
    }

    for target in &config.foundation.install {
        match target.as_str() {
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
            other => {
                eprintln!("warning: unsupported foundation target `{other}` in config; skipping");
            }
        }
    }

    Ok(())
}
