//! On-demand language-server installation for the built-in language pack.
//!
//! Starters prefer a server already on `PATH`, then a local install under
//! `<config>/i3-editor/lsp/`. Downloads are pinned and checksum-verified;
//! package-manager installs go through `npm` or a Python virtualenv. Nothing
//! runs until the `lsp_install` command is invoked.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::LspServerConfig;

const LUA_VERSION: &str = "3.19.2";
const NPM_TYPESCRIPT: &str = "typescript@5.9.3";
const NPM_TYPESCRIPT_LS: &str = "typescript-language-server@5.3.0";
const NPM_YAML_LS: &str = "yaml-language-server@1.19.2";
const NPM_DOCKER_LS: &str = "dockerfile-language-server-nodejs@0.15.0";
const PIP_PYTHON_LS: &str = "python-lsp-server==1.15.0";

/// npm/pip may take minutes on a cold cache; anything past this is hung.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug)]
pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    pub extensions: &'static [&'static str],
    pub filenames: &'static [&'static str],
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "rust",
        label: "Rust",
        extensions: &["rs"],
        filenames: &[],
    },
    Provider {
        id: "python",
        label: "Python",
        extensions: &["py", "pyi"],
        filenames: &[],
    },
    Provider {
        id: "typescript",
        label: "JavaScript / TypeScript",
        extensions: &["js", "jsx", "mjs", "cjs", "ts", "tsx"],
        filenames: &[],
    },
    Provider {
        id: "lua",
        label: "Lua",
        extensions: &["lua"],
        filenames: &[],
    },
    Provider {
        id: "yaml",
        label: "YAML",
        extensions: &["yml", "yaml"],
        filenames: &[],
    },
    Provider {
        id: "dockerfile",
        label: "Dockerfile",
        extensions: &["dockerfile"],
        filenames: &[
            "Dockerfile",
            "Containerfile",
            "Dockerfile.*",
            "Containerfile.*",
            "*.Dockerfile",
            "*.Containerfile",
        ],
    },
];

pub fn provider_for_path(path: &Path) -> Option<&'static Provider> {
    let filename = path.file_name().and_then(|name| name.to_str());
    for provider in PROVIDERS {
        if filename.is_some_and(|filename| {
            provider
                .filenames
                .iter()
                .any(|expected| crate::config::filename_matches(expected, filename))
        }) {
            return Some(provider);
        }
    }
    let extension = path.extension()?.to_str()?;
    PROVIDERS.iter().find(|provider| {
        provider
            .extensions
            .iter()
            .any(|expected| expected.eq_ignore_ascii_case(extension))
    })
}

pub enum Event {
    Status(String),
    Done {
        key: String,
        server: LspServerConfig,
    },
    Failed {
        key: String,
        message: String,
    },
}

pub struct Task {
    pub label: &'static str,
    pub rx: Receiver<Event>,
}

pub fn spawn(provider: &'static Provider, directory: PathBuf) -> Task {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let key = provider.id.to_string();
        let result = install(provider, &directory, &tx);
        let event = match result {
            Ok(server) => Event::Done { key, server },
            Err(message) => Event::Failed { key, message },
        };
        let _ = tx.send(event);
    });
    Task {
        label: provider.label,
        rx,
    }
}

fn install(
    provider: &Provider,
    directory: &Path,
    tx: &mpsc::Sender<Event>,
) -> Result<LspServerConfig, String> {
    match provider.id {
        "rust" => install_rust(provider, tx),
        "python" => install_python(provider, directory, tx),
        "typescript" => install_npm(
            provider,
            directory,
            &[NPM_TYPESCRIPT, NPM_TYPESCRIPT_LS],
            "typescript-language-server",
            tx,
        ),
        "yaml" => install_npm(
            provider,
            directory,
            &[NPM_YAML_LS],
            "yaml-language-server",
            tx,
        ),
        "dockerfile" => install_npm(
            provider,
            directory,
            &[NPM_DOCKER_LS],
            "docker-langserver",
            tx,
        ),
        "lua" => install_lua(provider, directory, tx),
        other => Err(format!("unknown language provider {other}")),
    }
}

fn install_rust(provider: &Provider, tx: &mpsc::Sender<Event>) -> Result<LspServerConfig, String> {
    if let Some(command) = find_on_path("rust-analyzer") {
        return Ok(server(provider, &command, &[]));
    }
    if find_on_path("rustup").is_none() {
        return Err(
            "rust-analyzer is not installed and rustup is missing; install Rust with rustup"
                .to_string(),
        );
    }
    status(tx, "installing rust-analyzer through rustup");
    run("rustup", &["component", "add", "rust-analyzer"])?;
    let command = run("rustup", &["which", "rust-analyzer"])?;
    Ok(server(provider, Path::new(command.trim()), &[]))
}

fn install_python(
    provider: &Provider,
    directory: &Path,
    tx: &mpsc::Sender<Event>,
) -> Result<LspServerConfig, String> {
    if let Some(command) = find_on_path("pylsp") {
        return Ok(server(provider, &command, &[]));
    }
    let venv = directory.join("python");
    let pylsp = venv.join("bin").join("pylsp");
    if !pylsp.is_file() {
        let python = find_on_path("python3")
            .or_else(|| find_on_path("python"))
            .ok_or_else(|| "Python is required to install pylsp".to_string())?;
        status(tx, "creating an isolated Python environment");
        let venv_arg = venv.to_string_lossy().into_owned();
        run_path(&python, &["-m", "venv", venv_arg.as_str()], None)?;
        let python = venv.join("bin").join("python");
        status(tx, "installing python-lsp-server with pip");
        run_path(
            &python,
            &[
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                PIP_PYTHON_LS,
            ],
            None,
        )?;
    }
    Ok(server(provider, &pylsp, &[]))
}

fn install_npm(
    provider: &Provider,
    directory: &Path,
    packages: &[&str],
    binary: &str,
    tx: &mpsc::Sender<Event>,
) -> Result<LspServerConfig, String> {
    if let Some(command) = find_on_path(binary) {
        return Ok(server(provider, &command, &["--stdio"]));
    }
    let prefix = directory.join(provider.id);
    let command = prefix.join("node_modules").join(".bin").join(binary);
    if !command.is_file() {
        let npm =
            find_on_path("npm").ok_or_else(|| format!("npm is required to install {binary}"))?;
        status(tx, &format!("installing {binary} with npm"));
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let mut args = vec![
            "install",
            "--prefix",
            prefix_arg.as_str(),
            "--no-audit",
            "--no-fund",
            "--omit=dev",
        ];
        args.extend_from_slice(packages);
        run_path(&npm, &args, None)?;
    }
    if !command.is_file() {
        return Err(format!("npm completed but {binary} was not created"));
    }
    Ok(server(provider, &command, &["--stdio"]))
}

fn install_lua(
    provider: &Provider,
    directory: &Path,
    tx: &mpsc::Sender<Event>,
) -> Result<LspServerConfig, String> {
    if let Some(command) = find_on_path("lua-language-server") {
        return Ok(server(provider, &command, &[]));
    }
    let (asset, checksum) = lua_asset()?;
    let root = directory.join("lua").join(LUA_VERSION);
    let command = root.join("bin").join("lua-language-server");
    if !command.is_file() {
        let archive = directory
            .join("lua")
            .join(format!("lua-language-server-{LUA_VERSION}.tar.gz"));
        let url = format!(
            "https://github.com/LuaLS/lua-language-server/releases/download/{LUA_VERSION}/lua-language-server-{LUA_VERSION}-{asset}.tar.gz"
        );
        if let Some(parent) = archive.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        status(
            tx,
            &format!("downloading Lua language server {LUA_VERSION}"),
        );
        run("curl", &["-fL", "--retry", "2", "-o", path(&archive), &url])?;
        let actual = crate::update::sha256_file(&archive).map_err(|error| error.to_string())?;
        if !actual.eq_ignore_ascii_case(checksum) {
            let _ = fs::remove_file(&archive);
            return Err("Lua language server checksum mismatch".to_string());
        }
        status(tx, "extracting Lua language server");
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        run("tar", &["-xzf", path(&archive), "-C", path(&root)])?;
        let _ = fs::remove_file(&archive);
    }
    if !command.is_file() {
        return Err("Lua language server archive did not contain bin/lua-language-server".into());
    }
    Ok(server(provider, &command, &[]))
}

fn lua_asset() -> Result<(&'static str, &'static str), String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok((
            "darwin-arm64",
            "79149584320a23bd5fcd5bc31f47a1975acd8a4560a0e89036bb1016fa61687f",
        )),
        ("macos", "x86_64") => Ok((
            "darwin-x64",
            "6f2174ece5ec63bcd9d5b22b2c902445c1cd9045fafe48c62602448ebf4c09cc",
        )),
        ("linux", "aarch64") => Ok((
            "linux-arm64",
            "17b8e401a874440adeb649e5024c364f2bb9d15156c74a40e56d1ae9a1b2e4f2",
        )),
        ("linux", "x86_64") => Ok((
            "linux-x64",
            "93ad326b10f6c00911dd8c450fa098332dbd8f9f45d1052f5636a2d54479b947",
        )),
        (os, arch) => Err(format!("no Lua language server asset for {os}/{arch}")),
    }
}

fn server(provider: &Provider, command: &Path, args: &[&str]) -> LspServerConfig {
    LspServerConfig {
        command: command.to_string_lossy().into_owned(),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        extensions: provider
            .extensions
            .iter()
            .map(|extension| (*extension).to_string())
            .collect(),
        filenames: provider
            .filenames
            .iter()
            .map(|filename| (*filename).to_string())
            .collect(),
    }
}

fn status(tx: &mpsc::Sender<Event>, message: &str) {
    let _ = tx.send(Event::Status(message.to_string()));
}

fn find_on_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(binary))
        .find(|candidate| candidate.is_file())
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    run_path(Path::new(program), args, None)
}

fn run_path(program: &Path, args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    run_with_timeout(program, args, cwd, INSTALL_TIMEOUT)
}

/// Run a child with a deadline. Output goes to temp files instead of pipes so a
/// chatty installer cannot fill a pipe buffer and look hung, and a process that
/// never exits is killed instead of blocking the worker forever — which used to
/// leave "install already running" stuck for the rest of the session.
fn run_with_timeout(
    program: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<String, String> {
    let mut stdout = tempfile::tempfile().map_err(|error| format!("scratch: {error}"))?;
    let mut stderr = tempfile::tempfile().map_err(|error| format!("scratch: {error}"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::from(
            stdout
                .try_clone()
                .map_err(|error| format!("scratch: {error}"))?,
        ))
        .stderr(Stdio::from(
            stderr
                .try_clone()
                .map_err(|error| format!("scratch: {error}"))?,
        ));
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                let mut err = String::new();
                let _ = stdout.seek(SeekFrom::Start(0));
                let _ = stdout.read_to_string(&mut out);
                let _ = stderr.seek(SeekFrom::Start(0));
                let _ = stderr.read_to_string(&mut err);
                if !status.success() {
                    let first = err
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("");
                    return Err(format!("{} failed: {}", program.display(), first.trim()));
                }
                return Ok(out);
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} timed out after {}s",
                    program.display(),
                    timeout.as_secs()
                ));
            }
            Err(error) => return Err(format!("{}: {error}", program.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn a_hung_installer_is_killed_at_the_deadline() {
        let started = Instant::now();
        let error = run_with_timeout(
            Path::new("sh"),
            &["-c", "sleep 5"],
            None,
            Duration::from_millis(150),
        )
        .expect_err("must time out");
        assert!(error.contains("timed out"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the kill must happen at the deadline"
        );
    }

    #[test]
    fn providers_match_extensions_and_dockerfile_names() {
        assert_eq!(
            provider_for_path(Path::new("main.rs")).map(|p| p.id),
            Some("rust")
        );
        assert_eq!(
            provider_for_path(Path::new("app.py")).map(|p| p.id),
            Some("python")
        );
        assert_eq!(
            provider_for_path(Path::new("index.tsx")).map(|p| p.id),
            Some("typescript")
        );
        assert_eq!(
            provider_for_path(Path::new("init.lua")).map(|p| p.id),
            Some("lua")
        );
        assert_eq!(
            provider_for_path(Path::new("ci.yaml")).map(|p| p.id),
            Some("yaml")
        );
        assert_eq!(
            provider_for_path(Path::new("Dockerfile")).map(|p| p.id),
            Some("dockerfile")
        );
        assert_eq!(
            provider_for_path(Path::new("Containerfile")).map(|p| p.id),
            Some("dockerfile")
        );
        assert_eq!(
            provider_for_path(Path::new("Dockerfile.dev")).map(|p| p.id),
            Some("dockerfile")
        );
        assert_eq!(
            provider_for_path(Path::new("app.Dockerfile")).map(|p| p.id),
            Some("dockerfile")
        );
        assert!(provider_for_path(Path::new("notes.txt")).is_none());
    }
}
