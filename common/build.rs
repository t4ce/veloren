use regex::Regex;
use std::process::Command;

// Cargo's default package-file tracking excludes Git metadata. Separate target
// caches can therefore keep different version strings after the same commit.
fn watch_git_version() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=VELOREN_GIT_VERSION");
    for name in ["HEAD", "refs/heads", "refs/tags", "packed-refs"] {
        let Ok(output) = Command::new("git")
            .args(["rev-parse", "--path-format=absolute", "--git-path", name])
            .output()
        else {
            continue;
        };
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout);
            let path = path.trim();
            // Missing packed-refs is normal; watching an absent path would
            // make Cargo rerun the script on every build. Packing loose refs
            // changes refs/heads or refs/tags, which are watched above.
            if std::path::Path::new(path).exists() {
                println!("cargo::rerun-if-changed={path}");
            }
        }
    }
}

// Get the current githash+timestamp
// Note: It will compare commits. As long as the commits do not diverge from the
// server no version change will be detected.
fn get_git_hash_timestamp() -> Result<String, String> {
    let output = Command::new("git")
        .args(["log", "-n", "1", "--pretty=format:%h/%ct", "--abbrev=8"])
        .output()
        .map_err(|e| format!("Git version command couldn't be run with error: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Git version command was unsuccessful: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let hash_timestamp = String::from_utf8(output.stdout)
        .map_err(|e| format!("Git version command output isn't valid UTF-8: {}", e))?;
    let hash = hash_timestamp
        .split('/')
        .next()
        .ok_or("Git hash not found".to_string())?;
    // We only use the first 32 bits of the git hash
    if hash.len() != 8 {
        Ok(format!(
            "{}/{}",
            hash.get(..8)
                .ok_or("Git hash not long enough".to_string())?,
            hash_timestamp
                .split('/')
                .nth(1)
                .ok_or("Git timestamp not found".to_string())?
        ))
    } else {
        Ok(hash_timestamp)
    }
}

// Get the current gittag
fn get_git_tag() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--exact-match", "--tags", "HEAD"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let tag = String::from_utf8(output.stdout).ok()?.trim().to_string();

    if Regex::new(r"^v[0-9]+\.[0-9]+\.[0-9]+$")
        .unwrap()
        .is_match(&tag)
    {
        Some(tag)
    } else {
        None
    }
}

fn main() {
    watch_git_version();
    // If this env var exists, it'll be used instead
    if option_env!("VELOREN_GIT_VERSION").is_none() {
        let hash_timestamp = match get_git_hash_timestamp() {
            Ok(hash_timestamp) => hash_timestamp,
            Err(e) => {
                println!("cargo::error={}", e);
                println!(
                    "cargo::error=It is highly recommended to build Veloren from the cloned git \
                     repository with the git command available in order to give the game access \
                     to proper versioning information."
                );
                println!(
                    "cargo::error=However, if you wish to proceed building Veloren anyway, you \
                     can set the environment variable \"VELOREN_GIT_VERSION\" to \"/0/0\" before \
                     re-running the given cargo command (the specific procedure for this will \
                     depend on your shell). Note that this will compile the game with git commit \
                     hash and commit timestamp set to 0, which will cause version mismatch \
                     warnings where applicable, whether the version is actually mismatched or not."
                );
                return;
            },
        };

        let tag = get_git_tag().unwrap_or("".to_string());

        // Format: <git-tag?>/<git-hash>/<git-timestamp>
        println!(
            "cargo::rustc-env=VELOREN_GIT_VERSION={}/{}",
            tag, hash_timestamp
        );
    }
}
