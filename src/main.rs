use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;

use goh_tui::{err, info, ok, outln, step, warn};
use webcam_tune::camera::UvcCamera;
use webcam_tune::cli::{self, Command, Invocation, Lock};
use webcam_tune::profile::Profile;
use webcam_tune::settings::{self, Report};
use webcam_tune::{notify, profiles, watch};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match cli::parse(&args) {
        Ok(invocation) => invocation,
        Err(e) => {
            err(&format!("{e} (webcam-tune --help)"));
            return ExitCode::from(2);
        }
    };
    let result = match invocation.command {
        Command::Help => {
            outln(cli::HELP.trim_end());
            Ok(())
        }
        Command::Version => {
            outln(&format!("webcam-tune {}", cli::VERSION));
            Ok(())
        }
        Command::Apply => chosen(&invocation).and_then(|path| apply(&path)),
        Command::Capture => chosen(&invocation).and_then(|path| capture(&path)),
        Command::Lock(what) => chosen(&invocation).and_then(|path| lock(&path, what)),
        Command::Show => chosen(&invocation).and_then(|path| show(&path)),
        Command::Watch => run_watch(dir(&invocation)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if !e.is_empty() {
                err(&e);
            }
            ExitCode::FAILURE
        }
    }
}

fn dir(invocation: &Invocation) -> PathBuf {
    invocation.profiles_dir.clone().unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
        profiles::default_dir(
            std::env::var_os("XDG_CONFIG_HOME")
                .as_deref()
                .map(Path::new),
            &home,
        )
    })
}

fn chosen(invocation: &Invocation) -> Result<PathBuf, String> {
    profiles::resolve(&dir(invocation), invocation.profile.as_deref())
}

fn open(profile: &Profile) -> Result<UvcCamera, String> {
    UvcCamera::open_within(profile.device, &profile.agent).map_err(|e| e.0)
}

/// Apply one loaded profile and say how it went, in one line plus one per failure.
fn apply_profile(profile: &Profile, why: &str) -> Result<Report, String> {
    let report = settings::apply(&open(profile)?, &profile.controls);
    let stamp = cli::stamp();
    let label = format!("{} {}", profile.name, profile.version);
    if report.failed.is_empty() {
        ok(&format!(
            "{stamp} {why}: {label}: {} controls applied",
            report.applied.len()
        ));
    } else {
        let total = report.failed.len() + report.applied.len();
        err(&format!(
            "{stamp} {why}: {label}: {} of {total} controls did not take",
            report.failed.len()
        ));
        for failure in &report.failed {
            step(failure);
        }
    }
    for skipped in &report.skipped {
        warn(&format!("skipped {skipped}"));
    }
    Ok(report)
}

fn apply(path: &Path) -> Result<(), String> {
    let report = apply_profile(&Profile::load(path)?, "apply")?;
    if report.failed.is_empty() {
        Ok(())
    } else {
        Err(String::new())
    }
}

fn capture(path: &Path) -> Result<(), String> {
    let camera = open(&Profile::load(path)?)?;
    let saved = cli::capture(&camera, path)?;
    ok(&format!(
        "captured {} controls into {} ({} {})",
        saved.controls.iter().count(),
        path.display(),
        saved.name,
        saved.version
    ));
    Ok(())
}

fn lock(path: &Path, what: Lock) -> Result<(), String> {
    let camera = open(&Profile::load(path)?)?;
    let value = cli::lock(&camera, what).map_err(|e| e.0)?;
    ok(&format!("locked at {value}"));
    let saved = cli::capture(&camera, path)?;
    ok(&format!(
        "saved to {} ({} {})",
        path.display(),
        saved.name,
        saved.version
    ));
    Ok(())
}

fn show(path: &Path) -> Result<(), String> {
    let profile = Profile::load(path)?;
    info(&format!(
        "{} {} ({}), updated {}",
        profile.name,
        profile.version,
        path.display(),
        profile.updated
    ));
    let lines = cli::show(&open(&profile)?, &profile.controls).map_err(|e| e.0)?;
    for line in &lines {
        outln(line);
    }
    Ok(())
}

/// Apply what one burst of events asks for, from the profiles as they are on disk NOW, so an
/// edit takes effect at the next plug-in or wake with no restart.
fn handle_burst(dir: &Path, burst: &[watch::Event]) {
    let why = burst
        .iter()
        .map(|e| e.reason())
        .collect::<Vec<_>>()
        .join(", ");
    let stamp = || cli::stamp();
    let loaded = match profiles::load_all(dir) {
        Ok(loaded) => loaded,
        Err(e) => {
            err(&format!("{} {why}: {e}", stamp()));
            return;
        }
    };
    let targets = watch::targets(burst);
    for (_, profile) in &loaded {
        let wanted = targets
            .as_ref()
            .is_none_or(|devices| devices.contains(&profile.device));
        // After a wake every profile is a candidate, but an absent camera is not an error.
        let present = targets.is_some() || UvcCamera::present(profile.device);
        if wanted
            && present
            && let Err(e) = apply_profile(profile, &why)
        {
            warn(&format!("{} {why}: {}: {e}", stamp(), profile.name));
        }
    }
}

/// Sleep on the kernel notifications (main thread); apply once per burst (applier thread). The
/// cameras to watch are read once at start: a new profile file is watched after a restart.
fn run_watch(dir: PathBuf) -> Result<(), String> {
    let loaded = profiles::load_all(&dir)?;
    if loaded.is_empty() {
        return Err(format!(
            "no profiles in {}: run ./install.sh",
            dir.display()
        ));
    }
    let devices = loaded.iter().map(|(_, p)| p.device).collect::<Vec<_>>();
    let names = loaded
        .iter()
        .map(|(_, p)| p.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("applier".into())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                // Settle by the longest window any profile asks for; a broken profile directory
                // is reported on every event, never a silently dead applier.
                let settle = profiles::load_all(&dir)
                    .ok()
                    .and_then(|all| all.iter().map(|(_, p)| p.agent.settle).max())
                    .unwrap_or_default();
                let burst = watch::gather(first, &rx, settle);
                handle_burst(&dir, &burst);
            }
        })
        .map_err(|e| format!("cannot start the applier thread: {e}"))?;
    info(&format!(
        "{} watching for wake and for: {names}",
        cli::stamp()
    ));
    notify::run(devices, tx)
}
