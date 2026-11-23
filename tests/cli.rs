//! Process-level CLI contract tests. These never start the TUI.

use assert_cmd::Command;
use predicates::prelude::*;

fn command() -> Command {
    Command::cargo_bin("i3-editor").expect("binary target")
}

#[test]
fn version_prints_the_crate_version() {
    command()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "i3-editor {}",
            env!("CARGO_PKG_VERSION")
        )));
}

#[test]
fn help_lists_targets_and_options() {
    command()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("FILE:LINE"))
        .stdout(predicate::str::contains("--dump-keymap"))
        .stdout(predicate::str::contains("--update"));
}

#[test]
fn unknown_options_are_errors() {
    command()
        .arg("--udpate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown option --udpate"))
        .stdout(predicate::str::is_empty());
}

#[test]
fn mode_flags_reject_paths() {
    command()
        .args(["--update", "file.rs"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--update does not take paths"));
    command()
        .args(["--dump-keymap", "file.rs"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--dump-keymap does not take paths",
        ));
}

#[test]
fn file_line_rejects_multiple_targets() {
    command()
        .args(["one.rs:2", "two.rs"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("file:line accepts one file"));
}

#[test]
fn dump_keymap_works_without_a_terminal() {
    command()
        .arg("--dump-keymap")
        .assert()
        .success()
        .stdout(predicate::str::contains("keyboard_enhancement:"))
        .stdout(predicate::str::contains("theme:"));
}
