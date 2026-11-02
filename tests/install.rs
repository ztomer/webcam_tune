//! install.sh: the platform gate refuses every combination but Apple silicon macOS (both
//! directions, on one machine, through the `WEBCAM_TUNE_OS` / `WEBCAM_TUNE_ARCH` seam), and the `LaunchAgent` it
//! renders is a valid plist that runs the installed binary's `watch`.

#![cfg(test)]

use std::path::Path;
use std::process::{Command, Output, Stdio};

const SHIPPED: &str = include_str!("../profiles/kiyo-pro.toml");

fn install_sh(args: &[&str], os: &str, arch: &str) -> Output {
    Command::new("bash")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"))
        .args(args)
        .env("WEBCAM_TUNE_OS", os)
        .env("WEBCAM_TUNE_ARCH", arch)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn only_apple_silicon_macos_passes_the_platform_gate() {
    assert!(
        install_sh(&["--check-platform"], "Darwin", "arm64")
            .status
            .success()
    );
    for (os, arch) in [
        ("Darwin", "x86_64"),
        ("Linux", "x86_64"),
        ("Linux", "aarch64"),
        ("Darwin", "i386"),
    ] {
        let out = install_sh(&["--check-platform"], os, arch);
        assert!(!out.status.success(), "{os}/{arch} must be refused");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("unsupported"),
            "{os}/{arch} must say why"
        );
    }
}

#[test]
fn the_rendered_launch_agent_is_valid_and_runs_watch() {
    let out = install_sh(
        &[
            "--render-plist",
            "/opt/wt/bin/webcam-tune",
            "/tmp/wt.log",
            "/tmp/wt/profiles",
        ],
        "Darwin",
        "arm64",
    );
    assert!(out.status.success());
    let plist = String::from_utf8(out.stdout).unwrap();
    assert!(plist.contains(
        "<string>/opt/wt/bin/webcam-tune</string>\n        <string>watch</string>\n        \
             <string>--profiles-dir</string>\n        <string>/tmp/wt/profiles</string>"
    ));
    assert!(plist.contains("<key>KeepAlive</key>\n    <true/>"));
    let file = std::env::temp_dir().join(format!("webcam-agent-{}.plist", std::process::id()));
    std::fs::write(&file, &plist).unwrap();
    let lint = Command::new("plutil")
        .arg("-lint")
        .arg(&file)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    std::fs::remove_file(&file).unwrap();
    assert!(
        lint.status.success(),
        "{}",
        String::from_utf8_lossy(&lint.stdout)
    );
}

#[test]
fn an_unknown_option_is_bad_usage() {
    let out = install_sh(&["--frobnicate"], "Darwin", "arm64");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_full_install_and_uninstall_lands_and_removes_every_file() {
    // The whole path, not just the seams: a shell function named like the command it calls
    // (`install` calling `install -m`) once recursed forever here with every other test green.
    let root = std::env::temp_dir().join(format!("webcam-install-{}", std::process::id()));
    let (prefix, agents, xdg) = (root.join("prefix"), root.join("agents"), root.join("xdg"));
    let run = |args: &[&str]| {
        Command::new("bash")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"))
            .args(args)
            .env("PREFIX", &prefix)
            .env("LAUNCHAGENTS_DIR", &agents)
            .env("XDG_CONFIG_HOME", &xdg)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let installed = run(&[]);
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let bin = prefix.join("bin/webcam-tune");
    let version = Command::new(&bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("webcam-tune {}", env!("CARGO_PKG_VERSION"))
    );
    let plist = agents.join("io.github.ztomer.webcam-tune.plist");
    assert!(
        std::fs::read_to_string(&plist)
            .unwrap()
            .contains(&bin.display().to_string())
    );
    // Every shipped profile is seeded, and a second install keeps an edited one.
    let profile = xdg.join("webcam_tune/profiles/kiyo-pro.toml");
    assert_eq!(std::fs::read_to_string(&profile).unwrap(), SHIPPED);
    std::fs::write(&profile, "edited").unwrap();
    assert!(run(&[]).status.success());
    assert_eq!(std::fs::read_to_string(&profile).unwrap(), "edited");
    assert!(run(&["--uninstall"]).status.success());
    assert!(profile.exists(), "uninstall keeps the profiles");
    assert!(!bin.exists() && !plist.exists());
    std::fs::remove_dir_all(&root).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_one_line_installer_clones_the_newest_release_and_installs_from_it() {
    // `curl ... | bash`: the script arrives on stdin with no checkout beside it. The "remote" is
    // a repository made from this working tree, tagged v9.9.9 above an older v1.0.0.
    let root = std::env::temp_dir().join(format!("webcam-oneline-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&root));
    let remote = root.join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let listed = Command::new("git")
        .arg("-C")
        .arg(here)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    for file in String::from_utf8(listed.stdout).unwrap().lines() {
        let target = remote.join(file);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(here.join(file), target).unwrap();
    }
    git(&remote, &["init", "-q", "-b", "main"]);
    git(&remote, &["add", "-A"]);
    git(&remote, &["commit", "-q", "-m", "old"]);
    git(&remote, &["tag", "v1.0.0"]);
    std::fs::write(remote.join("NEWEST"), "").unwrap();
    git(&remote, &["add", "-A"]);
    git(&remote, &["commit", "-q", "-m", "new"]);
    git(&remote, &["tag", "v9.9.9"]);

    let script = std::fs::read(here.join("install.sh")).unwrap();
    let mut child = Command::new("bash")
        .current_dir(&root)
        .env("WEBCAM_TUNE_REPO", &remote)
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("PREFIX", root.join("prefix"))
        .env("LAUNCHAGENTS_DIR", root.join("agents"))
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), &script).unwrap();
    let out = child.wait_with_output().unwrap();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{log}");
    assert!(log.contains("using release v9.9.9"), "{log}");
    assert!(
        root.join("data/webcam_tune/src/NEWEST").exists(),
        "checked out the newest tag"
    );
    assert!(root.join("prefix/bin/webcam-tune").exists());
    assert!(
        root.join("config/webcam_tune/profiles/kiyo-pro.toml")
            .exists()
    );
    std::fs::remove_dir_all(&root).unwrap();
}
