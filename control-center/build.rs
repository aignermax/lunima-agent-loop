fn main() {
    println!("cargo:rerun-if-env-changed=LUNIMA_TEAM_VERSION");
    println!("cargo:rerun-if-changed=../.git");
    let version = std::env::var("LUNIMA_TEAM_VERSION")
        .ok()
        .or_else(|| {
            let output = std::process::Command::new("git")
                .args(["describe", "--tags", "--abbrev=0"])
                .output()
                .ok()?;
            output.status.success().then(|| {
                String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .trim_start_matches('v')
                    .to_string()
            })
        })
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=LUNIMA_TEAM_VERSION={version}");
}
