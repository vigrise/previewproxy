use anyhow::Result;
use semver::Version;

pub fn compare_versions(current: &str, latest: &str) -> std::cmp::Ordering {
    let cur = Version::parse(current).expect("invalid current version");
    let lat = Version::parse(latest).expect("invalid latest version");
    cur.cmp(&lat)
}

pub fn artifact_name() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "previewproxy-linux-x86_64";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "previewproxy-linux-arm64";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "previewproxy-darwin-x86_64";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "previewproxy-darwin-arm64";
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "previewproxy-windows-x86_64.exe";
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return "previewproxy-windows-arm64.exe";
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
    )))]
    compile_error!("unsupported target platform for self-upgrade");
}

pub fn download_url(tag: &str) -> String {
    format!(
        "https://github.com/vigrise/previewproxy/releases/download/v{}/{}",
        tag,
        artifact_name()
    )
}

pub async fn run_upgrade() -> Result<()> {
    use std::io::Write;
    use std::time::Duration;

    let exe = std::env::current_exe()?.canonicalize()?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot determine exe directory"))?;

    // Clean up leftover .old file from a previous Windows upgrade attempt
    #[cfg(target_os = "windows")]
    {
        let old = exe.with_extension("old");
        if old.exists() {
            let _ = std::fs::remove_file(&old);
        }
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("previewproxy-updater")
        .build()?;

    let resp = client
        .get("https://api.github.com/repos/vigrise/previewproxy/releases/latest")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Could not reach GitHub API: {e}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("GitHub API returned {}", resp.status());
    }

    let json: serde_json::Value = resp.json().await?;
    let tag = json["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing tag_name in GitHub API response"))?
        .trim_start_matches('v')
        .to_string();

    let current = env!("CARGO_PKG_VERSION");

    match compare_versions(current, &tag) {
        std::cmp::Ordering::Equal => {
            println!("Already up to date (v{current})");
            return Ok(());
        }
        std::cmp::Ordering::Greater => {
            println!(
                "Current version v{current} is newer than latest release v{tag}, skipping"
            );
            return Ok(());
        }
        std::cmp::Ordering::Less => {}
    }

    println!("Upgrading v{current} -> v{tag}...");

    let url = download_url(&tag);
    let download_resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Download failed: {e}"))?;

    if !download_resp.status().is_success() {
        anyhow::bail!("Download failed: server returned {}", download_resp.status());
    }

    // Write to a temp file in the same directory as the exe.
    // Required so that fs::rename stays on the same filesystem (avoids EXDEV error).
    let mut tmp = tempfile::Builder::new()
        .prefix(".previewproxy-upgrade-")
        .tempfile_in(exe_dir)
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to replace binary (try running with elevated permissions): {e}"
            )
        })?;

    let bytes = download_resp
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("Download failed: {e}"))?;
    tmp.write_all(&bytes)?;
    tmp.flush()?;

    // Set executable bit on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755))?;
    }

    let tmp_path = tmp.into_temp_path();

    #[cfg(unix)]
    {
        std::fs::rename(&tmp_path, &exe).map_err(|e| {
            anyhow::anyhow!(
                "Failed to replace binary (try running with elevated permissions): {e}"
            )
        })?;
        // File has been renamed away; forget TempPath so its destructor does not
        // attempt to delete a path that no longer exists.
        std::mem::forget(tmp_path);
    }

    #[cfg(windows)]
    {
        // Windows cannot replace a running exe. Rename current to .old first,
        // then move new file into the original path.
        let old = exe.with_extension("old");
        std::fs::rename(&exe, &old).map_err(|e| {
            anyhow::anyhow!(
                "Failed to replace binary (try running with elevated permissions): {e}"
            )
        })?;
        std::fs::rename(&tmp_path, &exe).map_err(|e| {
            anyhow::anyhow!(
                "Failed to replace binary (try running with elevated permissions): {e}"
            )
        })?;
        std::mem::forget(tmp_path);
    }

    println!("Upgraded to v{tag}. Restart to apply.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn test_same_version() {
        assert_eq!(compare_versions("1.3.0", "1.3.0"), Ordering::Equal);
    }

    #[test]
    fn test_latest_is_newer() {
        assert_eq!(compare_versions("1.3.0", "1.4.0"), Ordering::Less);
    }

    #[test]
    fn test_current_is_newer() {
        assert_eq!(compare_versions("1.4.0", "1.3.0"), Ordering::Greater);
    }

    #[test]
    fn test_semver_ordering_correctness() {
        // Must be semver comparison, not lexicographic (1.9 < 1.10)
        assert_eq!(compare_versions("1.9.0", "1.10.0"), Ordering::Less);
    }

    #[test]
    fn test_artifact_name_nonempty() {
        assert!(!artifact_name().is_empty());
    }

    #[test]
    fn test_download_url_format() {
        let url = download_url("1.4.0");
        assert!(
            url.starts_with("https://github.com/vigrise/previewproxy/releases/download/v1.4.0/"),
            "unexpected url: {url}"
        );
        assert!(url.contains("previewproxy-"), "unexpected url: {url}");
    }
}
