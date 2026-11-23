//! Command-line parsing: strict options plus file and `file:line` targets.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::app::OpenTarget;
use crate::error::{Error, Result};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub targets: Vec<OpenTarget>,
    pub update: bool,
    pub dump_keymap: bool,
    pub version: bool,
    pub help: bool,
}

pub fn parse() -> Result<Options> {
    parse_from(std::env::args_os().skip(1))
}

fn parse_from<I>(args: I) -> Result<Options>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = pico_args::Arguments::from_vec(args.into_iter().collect());
    let help = args.contains(["-h", "--help"]);
    let version = args.contains(["-V", "--version"]);
    let update = args.contains("--update");
    let dump_keymap = args.contains("--dump-keymap");

    let mut targets = Vec::new();
    for argument in args.finish() {
        let text = argument.to_string_lossy();
        if text.starts_with('-') {
            return Err(Error::Cli(format!("unknown option {text}")));
        }
        targets.push(parse_target(&text)?);
    }
    if update && !targets.is_empty() {
        return Err(Error::Cli("--update does not take paths".to_string()));
    }
    if dump_keymap && !targets.is_empty() {
        return Err(Error::Cli("--dump-keymap does not take paths".to_string()));
    }
    if targets.len() > 1 && targets.iter().any(|target| target.line.is_some()) {
        return Err(Error::Cli(
            "file:line accepts one file; open multiple files without line suffixes".to_string(),
        ));
    }

    Ok(Options {
        targets,
        update,
        dump_keymap,
        version,
        help,
    })
}

fn parse_target(text: &str) -> Result<OpenTarget> {
    if let Some((path, line)) = text.rsplit_once(':')
        && !line.is_empty()
        && line.bytes().all(|byte| byte.is_ascii_digit())
    {
        if path.is_empty() {
            return Err(Error::Cli(format!("invalid target {text:?}")));
        }
        let line: usize = line
            .parse()
            .map_err(|_| Error::Cli(format!("invalid line number in {text:?}")))?;
        if line == 0 {
            return Err(Error::Cli("line numbers start at 1".to_string()));
        }
        return Ok(OpenTarget {
            path: PathBuf::from(path),
            line: Some(line),
        });
    }
    Ok(OpenTarget {
        path: PathBuf::from(text),
        line: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options> {
        parse_from(args.iter().map(OsString::from))
    }

    #[test]
    fn rejects_unknown_options() {
        let error = parse_args(&["--udpate"]).expect_err("typo");
        assert!(error.to_string().contains("unknown option"), "{error}");
    }

    #[test]
    fn accepts_multiple_files() {
        let options = parse_args(&["a.rs", "b.rs"]).expect("parse");
        assert_eq!(
            options.targets,
            vec![
                OpenTarget {
                    path: PathBuf::from("a.rs"),
                    line: None
                },
                OpenTarget {
                    path: PathBuf::from("b.rs"),
                    line: None
                }
            ]
        );
    }

    #[test]
    fn parses_file_line_targets() {
        let options = parse_args(&["src/main.rs:42"]).expect("parse");
        assert_eq!(options.targets.len(), 1);
        assert_eq!(options.targets[0].path, PathBuf::from("src/main.rs"));
        assert_eq!(options.targets[0].line, Some(42));
        assert!(parse_args(&["src/main.rs:0"]).is_err());
        assert!(parse_args(&["one.rs:2", "two.rs"]).is_err());
    }

    #[test]
    fn mode_flags_reject_paths() {
        assert!(parse_args(&["--update", "file.rs"]).is_err());
        assert!(parse_args(&["--dump-keymap", "file.rs"]).is_err());
    }

    #[test]
    fn help_and_version_are_flags() {
        let options = parse_args(&["--help", "--version"]).expect("parse");
        assert!(options.help);
        assert!(options.version);
        assert!(options.targets.is_empty());
    }
}
