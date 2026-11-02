//! The binary as a user runs it: `--help` and `--version` work anywhere on the line, bad usage
//! exits 2, and a missing or broken profile is refused with the way forward rather than a guess.

#![cfg(test)]

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn webcam_tune(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_webcam-tune"))
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("webcam-bin-{name}-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&dir));
    dir
}

const SHIPPED: &str = include_str!("../profiles/kiyo-pro.toml");

#[test]
fn help_and_version_print_and_succeed() {
    for flag in ["--help", "-h", "help"] {
        let out = webcam_tune(&[flag]);
        assert!(out.status.success(), "{flag}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.starts_with("webcam-tune: keep a UVC webcam"),
            "{flag}: {text}"
        );
        assert!(
            text.contains("--version") && text.contains("--profile"),
            "{flag}"
        );
    }
    for flag in ["--version", "-V"] {
        let out = webcam_tune(&[flag]);
        assert!(out.status.success(), "{flag}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            format!("webcam-tune {}", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn bad_usage_exits_2_and_points_at_help() {
    for args in [&[][..], &["frobnicate"][..], &["apply", "--force"][..]] {
        let out = webcam_tune(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("webcam-tune --help"),
            "{args:?}"
        );
    }
}

#[test]
fn every_command_refuses_a_missing_profile_directory_with_the_way_to_make_one() {
    let dir = scratch("missing");
    let dir = dir.to_str().unwrap();
    for command in [
        &["apply"][..],
        &["capture"],
        &["show"],
        &["watch"],
        &["lock", "focus"],
    ] {
        let args: Vec<&str> = command
            .iter()
            .copied()
            .chain(["--profiles-dir", dir])
            .collect();
        let out = webcam_tune(&args);
        assert_eq!(out.status.code(), Some(1), "{command:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("no profiles at") && stderr.contains("run ./install.sh"),
            "{command:?}: {stderr}"
        );
    }
}

#[test]
fn a_broken_profile_is_refused_naming_the_key() {
    let dir = scratch("broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("kiyo-pro.toml"),
        SHIPPED.replace("retry-ms = 250", ""),
    )
    .unwrap();
    let out = webcam_tune(&["show", "--profiles-dir", dir.to_str().unwrap()]);
    let watch = webcam_tune(&["watch", "--profiles-dir", dir.to_str().unwrap()]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing agent.retry-ms"));
    assert_eq!(
        watch.status.code(),
        Some(1),
        "the agent refuses to start on a broken profile"
    );
}

#[test]
fn several_profiles_need_a_choice_and_an_unknown_name_lists_the_choices() {
    let dir = scratch("several");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("kiyo-pro.toml"), SHIPPED).unwrap();
    std::fs::write(
        dir.join("other.toml"),
        SHIPPED.replace("product-id = 0x0E05", "product-id = 0x0E08"),
    )
    .unwrap();
    let d = dir.to_str().unwrap();
    let unchosen = webcam_tune(&["show", "--profiles-dir", d]);
    let unknown = webcam_tune(&["show", "--profiles-dir", d, "--profile", "brio"]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        String::from_utf8_lossy(&unchosen.stderr)
            .contains("pick one with --profile (kiyo-pro, other)")
    );
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("no profile \"brio\""));
}
