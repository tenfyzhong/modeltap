use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
    println!("cargo:rerun-if-changed=.git/packed-refs");
    println!("cargo:rerun-if-changed=build.rs");

    let version = resolve_version();
    println!("cargo:rustc-env=MODELTAP_VERSION={version}");
}

fn resolve_version() -> String {
    if let Some(git_version) = version_from_git() {
        return git_version;
    }

    env!("CARGO_PKG_VERSION").to_string()
}

fn version_from_git() -> Option<String> {
    if let Ok(output) = Command::new("git")
        .args(["describe", "--tags", "--exact-match"])
        .output()
    {
        if output.status.success() {
            if let Ok(version) = String::from_utf8(output.stdout) {
                let trimmed = version.trim();
                if !trimmed.is_empty() {
                    return Some(clean_version(trimmed));
                }
            }
        }
    }

    if let Ok(output) = Command::new("git")
        .args(["describe", "--tags", "--always"])
        .output()
    {
        if output.status.success() {
            if let Ok(version) = String::from_utf8(output.stdout) {
                let trimmed = version.trim();
                if !trimmed.is_empty() {
                    return Some(clean_version(trimmed));
                }
            }
        }
    }

    None
}

fn clean_version(version: &str) -> String {
    version.strip_prefix('v').unwrap_or(version).to_string()
}
