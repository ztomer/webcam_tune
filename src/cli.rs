//! Argument parsing and the commands that need nothing but a camera, so the tests can hand them
//! a fake one.

use std::path::{Path, PathBuf};

use crate::camera::{Error, Uvc};
use crate::controls::{Value, find};
use crate::profile::Profile;
use crate::settings::{self, Settings};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const HELP: &str = "\
webcam-tune: keep a UVC webcam on one set of image settings on macOS

USAGE
    webcam-tune <COMMAND> [--profile <NAME|PATH>] [--profiles-dir <DIR>]

COMMANDS
    apply           write the profile's [controls] to its camera and read every value back
    capture         rewrite the profile's [controls] with what the camera holds now
    lock focus      freeze the current autofocus position as manual focus, then capture
    lock wb         freeze the current auto white balance as a manual value, then capture
    show            the camera's value and the profile's, control by control
    watch           apply every profile when its camera is plugged in, and all of them on wake
                    (what the LaunchAgent runs)

OPTIONS
    --profile <NAME|PATH>  a profile name (kiyo-pro) or a .toml path; may be left out when the
                           directory holds exactly one profile
    --profiles-dir <DIR>   default: $XDG_CONFIG_HOME/webcam_tune/profiles, else
                           ~/.config/webcam_tune/profiles. One file per camera, holding every
                           setting the program uses for it; the installer seeds it from the
                           repo's profiles/ without overwriting.
    -h, --help             print this help
    -V, --version          print the version

EXIT STATUS
    0 done, 1 the camera did not take the profile (each control is named), 2 bad usage
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lock {
    Focus,
    WhiteBalance,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Apply,
    Capture,
    Lock(Lock),
    Show,
    Watch,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub profile: Option<String>,
    pub profiles_dir: Option<PathBuf>,
}

/// # Errors
/// An unknown command or option, or a missing argument; the message says which.
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut words = Vec::new();
    let (mut profile, mut profiles_dir) = (None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" => {
                return Ok(Invocation {
                    command: Command::Help,
                    profile: None,
                    profiles_dir: None,
                });
            }
            "-V" | "--version" => {
                return Ok(Invocation {
                    command: Command::Version,
                    profile: None,
                    profiles_dir: None,
                });
            }
            "--profile" => {
                profile = Some(it.next().ok_or("--profile needs a name or path")?.clone());
            }
            "--profiles-dir" => {
                profiles_dir = Some(PathBuf::from(
                    it.next().ok_or("--profiles-dir needs a path")?,
                ));
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            word => words.push(word),
        }
    }
    let command = match words.as_slice() {
        ["apply"] => Command::Apply,
        ["capture"] => Command::Capture,
        ["lock", "focus"] => Command::Lock(Lock::Focus),
        ["lock", "wb"] => Command::Lock(Lock::WhiteBalance),
        ["lock", ..] => return Err("lock takes one of: focus, wb".into()),
        ["show"] => Command::Show,
        ["watch"] => Command::Watch,
        [] => return Err("no command given".into()),
        other => return Err(format!("unknown command: {}", other.join(" "))),
    };
    Ok(Invocation {
        command,
        profile,
        profiles_dir,
    })
}

/// Rewrite the profile's `[controls]` with what the camera holds. The profile's version moves up
/// one patch and `updated` becomes today; everything else in it stays.
///
/// # Errors
/// No readable profile at `path`, the camera would not read, or the file could not be written.
pub fn capture(camera: &dyn Uvc, path: &Path) -> Result<Profile, String> {
    let mut profile = Profile::load(path)?;
    profile.controls = settings::read(camera).map_err(|e| e.0)?;
    profile.version = profile.version.next_patch();
    let now = stamp();
    profile.updated = today();
    let header = format!(
        "webcam_tune profile. [controls] captured from the camera {now} by webcam-tune {VERSION}.\n\n\
         Every value the program uses for this camera is in this file; a missing key is an error\n\
         that names it. The LaunchAgent re-applies [controls] on every plug-in and wake. Tune the\n\
         picture in any UVC app, then run `webcam-tune capture` again."
    );
    std::fs::write(path, profile.render(&header))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(profile)
}

/// Turn the auto mode off at the value it chose, so the picture stops drifting.
///
/// # Errors
/// The camera refused a step; nothing is left half-locked silently, the message names the step.
pub fn lock(camera: &dyn Uvc, what: Lock) -> Result<Value, Error> {
    let (auto, manual, step) = match what {
        Lock::Focus => ("auto-focus", "focus-abs", 1),
        Lock::WhiteBalance => ("auto-white-balance-temp", "white-balance-temp", 10),
    };
    let (auto, manual) = (
        find(auto).ok_or_else(|| Error(auto.into()))?,
        find(manual).ok_or_else(|| Error(manual.into()))?,
    );
    let current = match camera.read(manual)? {
        Value::Int(n) => Value::Int((n + step / 2) / step * step),
        other => other,
    };
    camera.write(auto, Value::Bool(false))?;
    camera.write(manual, current)?;
    match camera.read(manual)? {
        got if got == current => Ok(current),
        got => Err(Error(format!(
            "{} = {current}: the camera holds {got}",
            manual.name
        ))),
    }
}

/// One line per control the camera or the profile mentions: `name  camera  profile`.
///
/// # Errors
/// The camera would not read.
pub fn show(camera: &dyn Uvc, wanted: &Settings) -> Result<Vec<String>, Error> {
    let held = settings::read(camera)?;
    let mut lines = vec![format!("{:<24} {:<22} {}", "control", "camera", "profile")];
    for control in &crate::controls::CONTROLS {
        let (have, want) = (held.get(control.name), wanted.get(control.name));
        if have.is_none() && want.is_none() {
            continue;
        }
        let cell =
            |v: Option<Value>, none: &str| v.map_or_else(|| none.to_string(), |v| v.to_string());
        let mark = if want.is_some() && want != have {
            "  <- differs"
        } else {
            ""
        };
        lines.push(format!(
            "{:<24} {:<22} {}{mark}",
            control.name,
            cell(have, "(auto)"),
            cell(want, "-")
        ));
    }
    Ok(lines)
}

/// Now, in the Mac's own time zone, as RFC 3339 with its offset: what log lines carry.
#[must_use]
pub fn stamp() -> String {
    jiff::Zoned::now()
        .strftime("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

/// Today's date in the Mac's own time zone, `YYYY-MM-DD`: what a profile's `updated` records.
#[must_use]
pub fn today() -> String {
    jiff::Zoned::now().date().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::fake;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn help_and_version_win_wherever_they_appear() {
        for line in [
            "--help",
            "-h",
            "help",
            "apply --help",
            "watch -h --profile x",
        ] {
            assert_eq!(parse(&args(line)).unwrap().command, Command::Help, "{line}");
        }
        for line in ["--version", "-V", "show -V"] {
            assert_eq!(
                parse(&args(line)).unwrap().command,
                Command::Version,
                "{line}"
            );
        }
        assert!(HELP.contains("--help") && HELP.contains("--version"));
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn commands_and_the_profile_options_parse() {
        let got = parse(&args("apply --profile kiyo-pro --profiles-dir /tmp/p")).unwrap();
        assert_eq!(
            got,
            Invocation {
                command: Command::Apply,
                profile: Some("kiyo-pro".into()),
                profiles_dir: Some("/tmp/p".into()),
            }
        );
        assert_eq!(
            parse(&args("lock wb")).unwrap().command,
            Command::Lock(Lock::WhiteBalance)
        );
        assert_eq!(
            parse(&args("lock focus")).unwrap().command,
            Command::Lock(Lock::Focus)
        );
        for (line, command) in [
            ("capture", Command::Capture),
            ("show", Command::Show),
            ("watch", Command::Watch),
        ] {
            assert_eq!(parse(&args(line)).unwrap().command, command);
        }
    }

    #[test]
    fn bad_usage_says_what_is_wrong() {
        assert_eq!(parse(&args("")).unwrap_err(), "no command given");
        assert_eq!(
            parse(&args("apply --profile")).unwrap_err(),
            "--profile needs a name or path"
        );
        assert_eq!(
            parse(&args("apply --profiles-dir")).unwrap_err(),
            "--profiles-dir needs a path"
        );
        assert_eq!(
            parse(&args("apply --force")).unwrap_err(),
            "unknown option --force"
        );
        assert_eq!(
            parse(&args("lock iris")).unwrap_err(),
            "lock takes one of: focus, wb"
        );
        assert_eq!(
            parse(&args("zoom in")).unwrap_err(),
            "unknown command: zoom in"
        );
    }

    #[test]
    fn capture_rewrites_the_controls_bumps_the_version_and_keeps_the_rest() {
        let dir = std::env::temp_dir().join(format!("webcam-capture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kiyo-pro.toml");
        let custom = crate::profile::SHIPPED.replace("settle-ms = 1500", "settle-ms = 900");
        std::fs::write(&path, custom).unwrap();
        let camera = fake::Camera::default();
        let captured = capture(&camera, &path).unwrap();
        let reloaded = Profile::load(&path).unwrap();
        assert_eq!(reloaded, captured);
        assert_eq!(reloaded.agent.settle, std::time::Duration::from_millis(900));
        assert_eq!(reloaded.controls, settings::read(&camera).unwrap());
        assert_eq!(reloaded.version, crate::profile::Version(1, 0, 1));
        assert_eq!(reloaded.updated, today());
        assert_eq!(reloaded.name, "Razer Kiyo Pro (v1)");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn capture_without_a_profile_refuses_instead_of_inventing_one() {
        let path =
            std::env::temp_dir().join(format!("webcam-none-{}/kiyo-pro.toml", std::process::id()));
        let message = capture(&fake::Camera::default(), &path).unwrap_err();
        assert!(message.contains("no profile at"), "{message}");
        assert!(!path.exists());
    }

    #[test]
    fn lock_freezes_the_auto_value_rounded_to_the_controls_step() {
        let camera = fake::Camera::default();
        camera
            .state
            .borrow_mut()
            .insert("white-balance-temp", Value::Int(5137));
        assert_eq!(lock(&camera, Lock::WhiteBalance).unwrap(), Value::Int(5140));
        assert_eq!(
            camera.state.borrow()["auto-white-balance-temp"],
            Value::Bool(false)
        );
        assert_eq!(lock(&camera, Lock::Focus).unwrap(), Value::Int(128));
        assert_eq!(camera.state.borrow()["auto-focus"], Value::Bool(false));
    }

    #[test]
    fn show_marks_what_differs_and_says_auto_for_values_an_auto_mode_owns() {
        let camera = fake::Camera::default();
        let wanted = Settings::parse("[controls]\nbrightness = 100\ncontrast = 128\n").unwrap();
        let lines = show(&camera, &wanted).unwrap();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("brightness") && l.ends_with("100  <- differs"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("contrast") && l.ends_with("128"))
        );
        assert!(
            !lines.iter().any(|l| l.starts_with("focus-abs")),
            "neither side holds it"
        );
        let wanted = Settings::parse("[controls]\nauto-focus = false\nfocus-abs = 9\n").unwrap();
        assert!(
            show(&camera, &wanted)
                .unwrap()
                .iter()
                .any(|l| l.starts_with("focus-abs") && l.contains("(auto)"))
        );
    }

    #[test]
    fn dates_and_stamps_are_local_and_well_formed() {
        let day = today();
        assert_eq!(day.len(), 10);
        assert!(day.parse::<jiff::civil::Date>().is_ok(), "{day}");
        let now = stamp();
        assert!(now.starts_with(&day[..4]), "{now}");
        assert!(
            now.parse::<jiff::Timestamp>().is_ok(),
            "{now} must carry its offset"
        );
    }
}
