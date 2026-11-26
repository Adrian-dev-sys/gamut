//! Special K updates from SK's own repository feed (same source as Playnite's helper).
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

const FEED: &str = "https://sk-data.special-k.info/repository.json";
/// Release channels in the feed.
pub const CHANNELS: [&str; 3] = ["Website", "Discord", "Ancient"];

#[derive(Debug, Clone)]
pub struct Update {
    pub current: String,
    pub latest: String,
    pub notes: String,
    pub installer: String,
    pub sha256: String,
}

#[derive(Deserialize)]
struct Feed {
    #[serde(rename = "Main")]
    main: Repo,
}

#[derive(Deserialize)]
struct Repo {
    #[serde(rename = "Versions")]
    versions: Vec<Version>,
}

#[derive(Deserialize)]
struct Version {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Branches", default)]
    branches: Vec<String>,
    #[serde(rename = "ReleaseNotes", default)]
    notes: String,
    #[serde(rename = "Installer", default)]
    installer: String,
    #[serde(rename = "SHA256", default)]
    sha256: String,
}

/// Installed version: SpecialK64.dll's file version, e.g. "26.10.10.0".
pub fn installed_version(sk_dir: &Path) -> Option<String> {
    super::sys::file_version(&sk_dir.join("SpecialK64.dll"))
}

/// Compares dotted versions numerically ("26.10.10" == "26.10.10.0").
fn newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        let mut v: Vec<u64> = s.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect();
        while v.last() == Some(&0) {
            v.pop();
        }
        v
    };
    parse(latest) > parse(current)
}

/// The newest release on `channel`, if it is newer than what's installed. Network; call off the UI thread.
pub fn check(sk_dir: &Path, channel: &str) -> Result<Option<Update>> {
    let current = installed_version(sk_dir).unwrap_or_else(|| "0.0.0".into());
    crate::http::throttle(FEED);
    let feed: Feed = crate::http::agent()
        .get(&format!("{FEED}?t={}", crate::now()))
        .call()?
        .body_mut()
        .read_json()
        .context("reading Special K's update feed")?;
    let Some(v) = feed.main.versions.into_iter().find(|v| v.branches.iter().any(|b| b.eq_ignore_ascii_case(channel))) else {
        return Ok(None);
    };
    Ok(newer(&v.name, &current).then(|| Update {
        current,
        latest: v.name,
        notes: v.notes,
        installer: v.installer,
        sha256: v.sha256,
    }))
}

/// Downloads the installer, checks its SHA-256, stops SK's services and runs it (interactive);
/// returns once the installer exits.
pub fn install(sk_dir: &Path, u: &Update) -> Result<()> {
    if !u.installer.starts_with("https://") {
        bail!("unexpected installer URL: {}", u.installer);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .user_agent("gamut/0.1")
        .build()
        .into();
    let bytes = agent
        .get(&u.installer)
        .call()?
        .body_mut()
        .with_config()
        .limit(512 * 1024 * 1024)
        .read_to_vec()
        .context("downloading the Special K installer")?;
    use sha2::Digest;
    let hash: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if !hash.eq_ignore_ascii_case(u.sha256.trim()) {
        bail!("the downloaded installer's checksum doesn't match; not running it");
    }
    let path = std::env::temp_dir().join(format!("SpecialK_{}.exe", u.latest.replace(['/', '\\', ':'], "_")));
    std::fs::write(&path, &bytes)?;
    super::stop_all(sk_dir);
    log::info!("Special K: running installer {}", path.display());
    let r = std::process::Command::new(&path).status();
    let _ = std::fs::remove_file(&path);
    let st = r.context("starting the Special K installer")?;
    if !st.success() {
        bail!("the installer exited with {st}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn version_order() {
        assert!(newer("26.11.26", "26.10.10.0"));
        assert!(!newer("26.10.10", "26.10.10.0"));
        assert!(newer("26.10.10.1", "26.10.10"));
        assert!(!newer("25.1", "26.4.3.1"));
    }
}
