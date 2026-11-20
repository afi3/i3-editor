//! Self-update from GitHub releases.
//!
//! Network access goes through `curl`, extraction through `tar`, and the
//! downloaded archive is checked against the release's SHA-256 file before the
//! running binary is replaced. The replacement writes a sibling file and renames
//! it over the canonical executable, so a failed update never leaves a truncated
//! binary behind.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

const REPO: &str = "afi3/i3-editor";
const BIN: &str = "i3-editor";

pub fn run() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("i3-editor {current}");
    let target = asset_target(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        Error::Update(format!(
            "no release asset for {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let tag = latest_tag()?;
    if !is_newer(&tag, current) {
        if same_version(&tag, current) {
            println!("already up to date ({tag})");
        } else {
            println!("running {current}; latest release is {tag}");
        }
        return Ok(());
    }
    println!("updating {current} -> {tag}");

    let scratch = scratch_dir()?;
    let archive_name = format!("{BIN}-{target}.tar.gz");
    let base = release_base(&tag);
    let archive = scratch.path.join(&archive_name);
    let checksum = scratch.path.join(format!("{archive_name}.sha256"));
    download(&format!("{base}/{archive_name}"), &archive)?;
    download(&format!("{base}/{archive_name}.sha256"), &checksum)?;

    let expected = read_checksum(&checksum)?;
    let actual = sha256_file(&archive)?;
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(Error::Update(format!(
            "checksum mismatch for {archive_name}"
        )));
    }
    println!("checksum ok ({actual})");

    extract(&archive, &scratch.path)?;
    let new_binary = scratch.path.join(BIN);
    if !new_binary.is_file() {
        return Err(Error::Update(format!(
            "{archive_name} does not contain {BIN}"
        )));
    }
    let executable = std::env::current_exe().map_err(|error| Error::io("locate", BIN, error))?;
    let executable = std::fs::canonicalize(&executable).unwrap_or(executable);
    install_binary(&new_binary, &executable)?;
    println!("updated to {tag} ({})", executable.display());
    println!("run `{BIN} --version` to confirm");
    Ok(())
}

/// Release asset suffix for a Rust target.
fn asset_target(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// Override for mirrors and end-to-end tests; the default is the GitHub API.
fn api_url() -> String {
    std::env::var("I3_EDITOR_UPDATE_API")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"))
}

/// Override for mirrors and end-to-end tests; returns `<base>/<tag>`.
fn release_base(tag: &str) -> String {
    match std::env::var("I3_EDITOR_UPDATE_BASE") {
        Ok(base) => format!("{}/{tag}", base.trim_end_matches('/')),
        Err(_) => format!("https://github.com/{REPO}/releases/download/{tag}"),
    }
}

fn latest_tag() -> Result<String> {
    let url = api_url();
    let output = curl()
        .arg("-fsSL")
        .arg("--retry")
        .arg("2")
        .arg(url)
        .output()
        .map_err(curl_error)?;
    if !output.status.success() {
        return Err(Error::Update(format!(
            "cannot query the latest release: {}",
            stderr(&output.stderr)
        )));
    }
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| Error::Update(format!("invalid release response: {error}")))?;
    payload
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Update("release has no tag_name".to_string()))
}

/// True when `latest` is a strictly newer `major.minor.patch` than `current`.
/// Pre-release suffixes are ignored, so `0.2.0-alpha` does not replace `0.2.0`.
fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => {
            let latest_core = (latest.0, latest.1, latest.2);
            let current_core = (current.0, current.1, current.2);
            if latest_core != current_core {
                return latest_core > current_core;
            }
            match (latest.3.as_deref(), current.3.as_deref()) {
                (None, None) => false,
                // A final release supersedes its pre-releases.
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (Some(latest), Some(current)) => latest > current,
            }
        }
        _ => latest.trim_start_matches('v') != current,
    }
}

fn same_version(left: &str, right: &str) -> bool {
    match (parse_version(left), parse_version(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left.trim_start_matches('v') == right,
    }
}

fn parse_version(text: &str) -> Option<(u64, u64, u64, Option<String>)> {
    let text = text.trim().trim_start_matches('v');
    let (core, prerelease) = match text.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (text.split('+').next().unwrap_or(text), None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    let prerelease = prerelease.map(|pre| pre.split('+').next().unwrap_or(pre).to_string());
    Some((major, minor, patch, prerelease))
}

fn download(url: &str, path: &Path) -> Result<()> {
    let mut command = curl();
    command
        .arg("-fsSL")
        .arg("--retry")
        .arg("2")
        .arg("-o")
        .arg(path);
    if url.starts_with("https://") {
        command.arg("--proto").arg("=https");
    }
    let output = command.arg(url).output().map_err(curl_error)?;
    if !output.status.success() {
        return Err(Error::Update(format!(
            "download failed: {url}: {}",
            stderr(&output.stderr)
        )));
    }
    Ok(())
}

fn curl() -> Command {
    Command::new("curl")
}

fn curl_error(error: std::io::Error) -> Error {
    Error::Update(format!("curl is required for --update: {error}"))
}

fn stderr(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if text.is_empty() {
        "no details".to_string()
    } else {
        text.to_string()
    }
}

fn read_checksum(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path).map_err(|error| Error::io("read", path, error))?;
    let checksum = text
        .split_whitespace()
        .next()
        .filter(|value| value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .ok_or_else(|| Error::Update(format!("malformed checksum file {}", path.display())))?;
    Ok(checksum.to_string())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|error| Error::io("read", path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| Error::io("read", path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract(archive: &Path, directory: &Path) -> Result<()> {
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(directory)
        .output()
        .map_err(|error| Error::Update(format!("tar is required for --update: {error}")))?;
    if !output.status.success() {
        return Err(Error::Update(format!(
            "cannot extract {}: {}",
            archive.display(),
            stderr(&output.stderr)
        )));
    }
    Ok(())
}

/// Replace `current` with `new` without truncating the old file first.
fn install_binary(new: &Path, current: &Path) -> Result<()> {
    let parent = current.parent().ok_or_else(|| {
        Error::Update(format!(
            "cannot locate the directory of {}",
            current.display()
        ))
    })?;
    let temporary = parent.join(format!(".{BIN}.update-{}", std::process::id()));
    if let Err(error) = std::fs::copy(new, &temporary) {
        let _ = std::fs::remove_file(&temporary);
        return Err(Error::io("write", &temporary, error));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) =
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))
        {
            let _ = std::fs::remove_file(&temporary);
            return Err(Error::io("chmod", &temporary, error));
        }
    }
    if let Err(error) = std::fs::rename(&temporary, current) {
        let _ = std::fs::remove_file(&temporary);
        return Err(Error::io("replace", current, error));
    }
    Ok(())
}

struct Scratch {
    path: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn scratch_dir() -> Result<Scratch> {
    let path = std::env::temp_dir().join(format!("{BIN}-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).map_err(|error| Error::io("create", &path, error))?;
    Ok(Scratch { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("0.2.0-alpha", "0.2.0"));
        assert!(is_newer("0.2.0", "0.2.0-alpha"));
        assert!(is_newer("0.2.0-beta", "0.2.0-alpha"));
        assert!(same_version("v0.2.0", "0.2.0"));
        assert!(!same_version("0.2.0", "0.2.0-alpha"));
    }

    #[test]
    fn release_assets_match_the_workflow_targets() {
        assert_eq!(
            asset_target("macos", "aarch64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(asset_target("macos", "x86_64"), Some("x86_64-apple-darwin"));
        assert_eq!(
            asset_target("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            asset_target("linux", "aarch64"),
            Some("aarch64-unknown-linux-gnu")
        );
        assert_eq!(asset_target("windows", "x86_64"), None);
        assert_eq!(asset_target("linux", "riscv64"), None);
    }

    #[test]
    fn checksums_are_read_from_the_first_field() {
        let dir = std::env::temp_dir().join(format!("i3-checksum-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("asset.sha256");
        let hex = "a".repeat(64);
        std::fs::write(&path, format!("{hex}  asset.tar.gz\\n")).expect("write");
        assert_eq!(read_checksum(&path).expect("checksum"), hex);

        std::fs::write(&path, "not-a-checksum  asset.tar.gz\\n").expect("write");
        assert!(read_checksum(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn archive_is_extracted_then_installed() {
        let dir = std::env::temp_dir().join(format!("i3-archive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let payload = dir.join("payload");
        std::fs::create_dir_all(&payload).expect("payload");
        std::fs::write(payload.join(BIN), "fake-binary").expect("binary");

        let archive = dir.join("asset.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&payload)
            .arg(BIN)
            .status()
            .expect("tar");
        assert!(status.success());

        let checksum = sha256_file(&archive).expect("sha256");
        std::fs::write(
            dir.join("asset.sha256"),
            format!("{checksum}  asset.tar.gz\n"),
        )
        .expect("checksum");
        assert_eq!(
            read_checksum(&dir.join("asset.sha256")).expect("read"),
            checksum
        );

        let unpacked = dir.join("unpacked");
        std::fs::create_dir_all(&unpacked).expect("unpacked");
        extract(&archive, &unpacked).expect("extract");
        assert_eq!(
            std::fs::read_to_string(unpacked.join(BIN)).expect("read"),
            "fake-binary"
        );

        let current = dir.join("current");
        std::fs::write(&current, "old").expect("current");
        install_binary(&unpacked.join(BIN), &current).expect("install");
        assert_eq!(
            std::fs::read_to_string(&current).expect("read"),
            "fake-binary"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn installing_replaces_the_binary_and_sets_exec_mode() {
        let dir = std::env::temp_dir().join(format!("i3-install-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let current = dir.join(BIN);
        std::fs::write(&current, "old").expect("old");
        let new = dir.join("new");
        std::fs::write(&new, "new-binary").expect("new");

        install_binary(&new, &current).expect("install");
        assert_eq!(
            std::fs::read_to_string(&current).expect("read"),
            "new-binary"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&current)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("dir")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".update-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
