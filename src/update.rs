//! Self-update from GitHub releases.
//!
//! Network access goes through `curl`, extraction through `tar`, and the
//! downloaded archive is checked against the release's SHA-256 file before the
//! running binary is replaced. The replacement writes a sibling file and renames
//! it over the canonical executable, so a failed update never leaves a truncated
//! binary behind.

use std::io::Read;
use std::path::Path;
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
    let archive = scratch.path().join(&archive_name);
    let checksum = scratch.path().join(format!("{archive_name}.sha256"));
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

    extract(&archive, scratch.path())?;
    let new_binary = scratch.path().join(BIN);
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
        .arg("--connect-timeout")
        .arg("15")
        .arg("--max-time")
        .arg("600")
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
                (Some(latest), Some(current)) => compare_prerelease(latest, current).is_gt(),
            }
        }
        _ => latest.trim_start_matches('v') != current,
    }
}

/// SemVer precedence for dot-separated identifiers: numeric fields compare as
/// numbers, so `alpha.10` is newer than `alpha.9`.
fn compare_prerelease(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let mut left = left.split('.');
    let mut right = right.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left), Some(right)) => {
                let order = match (left.parse::<u64>(), right.parse::<u64>()) {
                    (Ok(left), Ok(right)) => left.cmp(&right),
                    // Numeric identifiers have lower precedence than text.
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => left.cmp(right),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
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
        .arg("--connect-timeout")
        .arg("15")
        .arg("--max-time")
        .arg("600")
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

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
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

/// A tar member may not be absolute or climb out of the target directory.
fn entry_is_safe(line: &str) -> bool {
    !line.starts_with('/') && !line.split('/').any(|part| part == "..")
}

/// Extract with a member check first. The checksum already ties the archive to
/// the release, but extraction is the step that writes to disk.
pub(crate) fn safe_extract(archive: &Path, directory: &Path) -> std::result::Result<(), String> {
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(archive)
        .output()
        .map_err(|error| format!("tar is required: {error}"))?;
    if !listing.status.success() {
        return Err(format!(
            "cannot list {}: {}",
            archive.display(),
            stderr(&listing.stderr)
        ));
    }
    let names = String::from_utf8_lossy(&listing.stdout);
    for line in names.lines().filter(|line| !line.trim().is_empty()) {
        if !entry_is_safe(line) {
            return Err(format!("refusing unsafe archive entry: {line}"));
        }
    }
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(directory)
        .arg("--no-same-owner")
        .output()
        .map_err(|error| format!("tar is required: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot extract {}: {}",
            archive.display(),
            stderr(&output.stderr)
        ));
    }
    Ok(())
}

fn extract(archive: &Path, directory: &Path) -> Result<()> {
    safe_extract(archive, directory).map_err(Error::Update)
}

/// Replace `current` with `new` without truncating the old file first. The
/// temporary file gets an unpredictable name so a pre-created symlink cannot
/// redirect the write.
fn install_binary(new: &Path, current: &Path) -> Result<()> {
    let parent = current.parent().ok_or_else(|| {
        Error::Update(format!(
            "cannot locate the directory of {}",
            current.display()
        ))
    })?;
    let mut source = std::fs::File::open(new).map_err(|error| Error::io("read", new, error))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".i3-editor-update-")
        .tempfile_in(parent)
        .map_err(|error| Error::io("write", parent, error))?;
    std::io::copy(&mut source, &mut temporary)
        .map_err(|error| Error::io("write", temporary.path(), error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))
            .map_err(|error| Error::io("chmod", temporary.path(), error))?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| Error::io("write", temporary.path(), error))?;
    temporary
        .persist(current)
        .map_err(|error| Error::io("replace", current, error.error))?;
    sync_dir(parent);
    Ok(())
}

/// Flush the directory entry so the rename survives a crash; best effort.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn scratch_dir() -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix("i3-editor-update-")
        .tempdir()
        .map_err(|error| Error::Update(format!("cannot create a scratch directory: {error}")))
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
        assert_eq!(asset_target("freebsd", "x86_64"), None);
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

#[cfg(test)]
mod prerelease_tests {
    use super::*;

    #[test]
    fn prereleases_compare_numerically() {
        assert!(is_newer("0.3.0-alpha.10", "0.3.0-alpha.9"));
        assert!(!is_newer("0.3.0-alpha.9", "0.3.0-alpha.10"));
        assert!(is_newer("0.3.0-beta", "0.3.0-alpha.10"));
        assert!(is_newer("0.3.0-alpha.1", "0.3.0-alpha"));
        // A final release supersedes its pre-release, never the other way.
        assert!(is_newer("0.3.0", "0.3.0-rc.1"));
        assert!(!is_newer("0.3.0-rc.1", "0.3.0"));
    }
}

#[cfg(test)]
mod archive_tests {
    use super::*;

    #[test]
    fn archive_members_may_not_escape_the_target() {
        assert!(entry_is_safe("i3-editor"));
        assert!(entry_is_safe("./bin/i3-editor"));
        assert!(!entry_is_safe("../escape"));
        assert!(!entry_is_safe("a/../../escape"));
        assert!(!entry_is_safe("/etc/passwd"));
    }
}
