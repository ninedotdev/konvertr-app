//! Self-update: check a published manifest, download the signed app tarball,
//! verify it, swap the bundle, relaunch. Modelled on comet's update crate.

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// The latest release's manifest. `KONVRT_MANIFEST_URL` overrides it for
/// testing against a draft or a local file.
pub const MANIFEST_URL: &str =
    "https://github.com/ninedotdev/konvertr-app/releases/latest/download/manifest.json";

/// Team ID the downloaded bundle must be signed by; an update signed by anyone
/// else is refused. `None` accepts any valid signature (ad-hoc dev builds).
pub const EXPECTED_TEAM_ID: Option<&str> = Some("3HV82GAPMK");

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn manifest_url() -> String {
    std::env::var("KONVRT_MANIFEST_URL").unwrap_or_else(|_| MANIFEST_URL.to_string())
}

/// `manifest.json`, written by the release workflow.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    /// Platform key (`macos-arm64`) → artifact.
    #[serde(default)]
    pub platforms: std::collections::BTreeMap<String, Artifact>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// `macos-arm64` / `macos-x86_64`, matching the release workflow's names.
pub fn platform_key() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("{}-{arch}", std::env::consts::OS)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub version: String,
    pub notes: Option<String>,
    pub artifact: Artifact,
}

/// Strictly-newer dotted-numeric compare. Unparseable versions never count as
/// newer, so a garbage manifest can't drive an update loop.
pub fn version_newer(latest: &str, current: &str) -> bool {
    fn parts(v: &str) -> Option<Vec<u64>> {
        let nums: Vec<u64> = v
            .trim()
            .trim_start_matches('v')
            .split('-')
            .next()?
            .split('.')
            .map(|p| p.parse::<u64>().ok())
            .collect::<Option<_>>()?;
        (!nums.is_empty()).then_some(nums)
    }
    let (Some(l), Some(c)) = (parts(latest), parts(current)) else {
        return false;
    };
    for i in 0..l.len().max(c.len()) {
        let (a, b) = (
            l.get(i).copied().unwrap_or(0),
            c.get(i).copied().unwrap_or(0),
        );
        if a != b {
            return a > b;
        }
    }
    false
}

/// Fetch the manifest and report an update when one applies to this platform.
pub fn check() -> Result<Option<Available>> {
    let body = ureq::get(&manifest_url())
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .context("fetching the update manifest")?
        .into_string()
        .context("reading the update manifest")?;
    Ok(pick(
        &serde_json::from_str(&body).context("parsing the update manifest")?,
    ))
}

/// Manifest → the update this build should install, if any.
pub fn pick(manifest: &Manifest) -> Option<Available> {
    if !version_newer(&manifest.version, current_version()) {
        return None;
    }
    let artifact = manifest.platforms.get(&platform_key())?.clone();
    Some(Available {
        version: manifest.version.clone(),
        notes: manifest.notes.clone(),
        artifact,
    })
}

/// The running `.app` bundle, walking up from the executable. `None` for a
/// plain `cargo run` binary, which must not self-update.
pub fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// Download the tarball, check its digest, unpack it, and verify the signature.
/// Returns the staged `.app` — nothing has touched the installed copy yet.
pub fn stage(artifact: &Artifact, progress: &Arc<AtomicU8>) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("konvrt-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).context("creating the staging directory")?;

    let tarball = dir.join("update.tar.gz");
    download(&artifact.url, &tarball, artifact.size, progress)?;

    if let Some(expected) = &artifact.sha256 {
        let actual = sha256_file(&tarball)?;
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("the download did not match its checksum");
        }
    }

    run(
        "tar",
        &[
            "-xzf",
            &tarball.to_string_lossy(),
            "-C",
            &dir.to_string_lossy(),
        ],
    )?;
    let staged = std::fs::read_dir(&dir)
        .context("reading the staging directory")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .context("the update did not contain an app bundle")?;

    verify_signature(&staged)?;
    Ok(staged)
}

/// Refuse anything that isn't intact and signed by us.
pub fn verify_signature(app: &Path) -> Result<()> {
    let out = std::process::Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(app)
        .output()
        .context("running codesign")?;
    if !out.status.success() {
        bail!("the update's signature did not verify");
    }
    let Some(team) = EXPECTED_TEAM_ID else {
        return Ok(());
    };
    let info = std::process::Command::new("codesign")
        .args(["-dv", "--verbose=4"])
        .arg(app)
        .output()
        .context("reading the update's signature")?;
    let text = String::from_utf8_lossy(&info.stderr);
    if !text.contains(&format!("TeamIdentifier={team}")) {
        bail!("the update was signed by someone else");
    }
    Ok(())
}

/// Swap the installed bundle for the staged one: copy next to the target, then
/// two renames, restoring the old bundle if the second fails.
pub fn apply(staged: &Path, bundle: &Path) -> Result<()> {
    let parent = bundle.parent().context("the app has no parent directory")?;
    let name = bundle
        .file_name()
        .context("the app has no name")?
        .to_string_lossy()
        .into_owned();
    let pid = std::process::id();
    let fresh = parent.join(format!(".{name}.new-{pid}"));
    let old = parent.join(format!(".{name}.old-{pid}"));
    let _ = std::fs::remove_dir_all(&fresh);

    run(
        "ditto",
        &[&staged.to_string_lossy(), &fresh.to_string_lossy()],
    )?;
    std::fs::rename(bundle, &old).context("moving the current app aside")?;
    if let Err(err) = std::fs::rename(&fresh, bundle) {
        let _ = std::fs::rename(&old, bundle);
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(err).context("installing the new app");
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Waits for this process to exit, then reopens the bundle. The caller quits
/// right after; opening earlier would race our own shutdown.
pub fn relaunch_after_exit(bundle: &Path) {
    use std::os::unix::process::CommandExt as _;
    let script = format!(
        "while /bin/kill -0 {} 2>/dev/null; do sleep 0.2; done; /usr/bin/open \"{}\"",
        std::process::id(),
        bundle.display()
    );
    let _ = std::process::Command::new("/bin/sh")
        .args(["-c", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn();
}

fn download(url: &str, to: &Path, size: Option<u64>, progress: &Arc<AtomicU8>) -> Result<()> {
    let response = ureq::get(url)
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let total = size.or_else(|| {
        response
            .header("content-length")
            .and_then(|l| l.parse::<u64>().ok())
    });

    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(to).context("creating the download file")?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = std::io::Read::read(&mut reader, &mut buf).context("reading the download")?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).context("writing the download")?;
        done += n as u64;
        if let Some(total) = total.filter(|t| *t > 0) {
            progress.store((done * 100 / total).min(100) as u8, Ordering::Relaxed);
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).context("opening the download")?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).context("hashing the download")?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions_win_and_garbage_never_does() {
        assert!(version_newer("0.2.0", "0.1.9"));
        assert!(version_newer("0.1.10", "0.1.9"));
        assert!(version_newer("v1.0.0", "0.9.9"));
        assert!(version_newer("0.1.1", "0.1"));
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.2.0"));
        assert!(!version_newer("nightly", "0.1.0"));
        assert!(!version_newer("", "0.1.0"));
    }

    fn manifest(version: &str, key: &str) -> Manifest {
        let mut platforms = std::collections::BTreeMap::new();
        platforms.insert(
            key.to_string(),
            Artifact {
                url: "https://example.test/app.tar.gz".into(),
                sha256: Some("abc".into()),
                size: Some(10),
            },
        );
        Manifest {
            version: version.to_string(),
            notes: Some("faster".into()),
            platforms,
        }
    }

    #[test]
    fn picks_only_newer_builds_for_this_platform() {
        let key = platform_key();
        assert!(pick(&manifest("99.0.0", &key)).is_some());
        assert!(pick(&manifest("0.0.1", &key)).is_none());
        // Newer, but nothing published for us.
        assert!(pick(&manifest("99.0.0", "windows-x86_64")).is_none());
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let key = platform_key();
        let json = serde_json::to_string(&manifest("9.9.9", &key)).unwrap();
        let parsed: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, "9.9.9");
        assert_eq!(parsed.platforms[&key].size, Some(10));
        // Optional fields may be absent entirely.
        let minimal: Manifest =
            serde_json::from_str(r#"{"version":"1.0.0","platforms":{}}"#).unwrap();
        assert_eq!(minimal.version, "1.0.0");
        assert!(minimal.notes.is_none());
    }

    /// Full staging path against the published release: download, checksum,
    /// unpack, signature check. Network-bound, so it stays out of the default
    /// run — `cargo test -p konvrt-core -- --ignored`.
    #[test]
    #[ignore]
    fn stages_the_published_release() {
        let body = ureq::get(MANIFEST_URL)
            .call()
            .expect("fetching the manifest")
            .into_string()
            .unwrap();
        let manifest: Manifest = serde_json::from_str(&body).expect("parsing the manifest");
        let artifact = manifest
            .platforms
            .get(&platform_key())
            .expect("no artifact for this platform");

        let progress = Arc::new(AtomicU8::new(0));
        let staged = stage(artifact, &progress).expect("staging the update");
        assert!(staged.join("Contents/MacOS").exists(), "not an app bundle");
        assert_eq!(progress.load(Ordering::Relaxed), 100);
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn platform_key_is_the_artifact_naming() {
        let key = platform_key();
        assert!(
            key.starts_with("macos-") || key.starts_with("linux-"),
            "{key}"
        );
        assert!(!key.contains("aarch64"), "arm64 is the release naming");
    }
}
