use crate::config::{DEFAULT_ESSENTIAL_PACKAGES, EssentialsConfig};

pub fn run(config: &EssentialsConfig) -> anyhow::Result<()> {
    let packages = selected_packages(config);
    if packages.is_empty() {
        println!("No essentials packages configured (`essentials.packages` is empty).");
        return Ok(());
    }

    let package_refs = packages.iter().map(String::as_str).collect::<Vec<_>>();
    let installed = super::apt::install_missing(&package_refs)?;
    if installed.is_empty() {
        println!("Essential packages already installed.");
    } else {
        println!("Installed essential packages: {}", installed.join(", "));
    }

    Ok(())
}

fn selected_packages(config: &EssentialsConfig) -> Vec<String> {
    if config.packages.is_empty() {
        return DEFAULT_ESSENTIAL_PACKAGES
            .iter()
            .map(|package| (*package).to_string())
            .collect();
    }

    config
        .packages
        .iter()
        .map(|package| package.trim())
        .filter(|package| !package.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_baseline_packages() {
        let config = EssentialsConfig {
            packages: Vec::new(),
        };
        assert_eq!(
            selected_packages(&config),
            DEFAULT_ESSENTIAL_PACKAGES
                .iter()
                .map(|package| (*package).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn trims_configured_packages() {
        let config = EssentialsConfig {
            packages: vec![" curl ".to_string(), "".to_string(), "wget".to_string()],
        };
        assert_eq!(selected_packages(&config), vec!["curl", "wget"]);
    }
}
