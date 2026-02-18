pub fn run() {
    println!("Available install/configure targets:");
    for target in super::targets() {
        let mut capabilities = Vec::new();
        if target.supports_install {
            capabilities.push("install");
        }
        if target.supports_configure {
            capabilities.push("configure");
        }

        println!(
            "- {} [{}]: {}",
            target.name,
            capabilities.join(", "),
            target.description
        );
    }
}
