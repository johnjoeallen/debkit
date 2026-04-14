use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;

const PROMPT_FILENAME: &str = ".git-prompt.sh";
const PROMPT_CONTENT: &str = r#"# Enable git prompt
if [ -f /usr/lib/git-core/git-sh-prompt ]; then
    source /usr/lib/git-core/git-sh-prompt
    export GIT_PS1_SHOWDIRTYSTATE=1
    export GIT_PS1_SHOWSTASHSTATE=1
    export GIT_PS1_SHOWUNTRACKEDFILES=1
    export GIT_PS1_SHOWUPSTREAM="auto"
    PS1='\u@\h:\w$(__git_ps1 " (%s)")\$ '
fi
"#;

pub fn run() -> anyhow::Result<()> {
    let home = home_dir()?;
    let prompt_path = home.join(PROMPT_FILENAME);
    let bashrc_path = home.join(".bashrc");

    if let Some(parent) = prompt_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let mut changed = false;

    let existing_prompt = fs::read_to_string(&prompt_path).unwrap_or_default();
    if existing_prompt != PROMPT_CONTENT {
        fs::write(&prompt_path, PROMPT_CONTENT)
            .with_context(|| format!("failed to write {}", prompt_path.display()))?;
        changed = true;
    }

    if !bashrc_path.exists() {
        fs::write(&bashrc_path, "")
            .with_context(|| format!("failed to create {}", bashrc_path.display()))?;
    }

    let bashrc_content = fs::read_to_string(&bashrc_path)
        .with_context(|| format!("failed to read {}", bashrc_path.display()))?;
    let source_block = format!(
        "# Load git prompt configuration\nif [ -f \"{}\" ]; then\n  . \"{}\"\nfi\n",
        prompt_path.display(),
        prompt_path.display()
    );

    if !bashrc_content.contains(prompt_path.to_string_lossy().as_ref()) {
        let mut handle = OpenOptions::new()
            .append(true)
            .open(&bashrc_path)
            .with_context(|| format!("failed to open {} for append", bashrc_path.display()))?;
        if !bashrc_content.is_empty() && !bashrc_content.ends_with('\n') {
            writeln!(handle)?;
        }
        if !bashrc_content.is_empty() {
            writeln!(handle)?;
        }
        write!(handle, "{source_block}")?;
        changed = true;
    }

    if changed {
        println!(
            "Configured git prompt: file={}, sourced via {}",
            prompt_path.display(),
            bashrc_path.display()
        );
    } else {
        println!("Git prompt already configured");
    }

    Ok(())
}

fn home_dir() -> anyhow::Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}
