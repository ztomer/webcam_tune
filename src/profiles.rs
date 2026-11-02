//! The profile directory: one file per camera, named by its file stem (`kiyo-pro.toml` is the
//! profile `kiyo-pro`).

use std::path::{Path, PathBuf};

use crate::profile::Profile;

/// `$XDG_CONFIG_HOME/webcam_tune/profiles`, else `~/.config/webcam_tune/profiles`.
#[must_use]
pub fn default_dir(xdg_config_home: Option<&Path>, home: &Path) -> PathBuf {
    xdg_config_home
        .map_or_else(|| home.join(".config"), Path::to_path_buf)
        .join("webcam_tune/profiles")
}

fn is_toml(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
}

/// Every `*.toml` in `dir`, as (name, path), sorted by name.
///
/// # Errors
/// The directory cannot be read (a missing one says how to create it).
pub fn list(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "no profiles at {}: run ./install.sh, or copy one from the repo's profiles/",
                dir.display()
            )
        } else {
            format!("{}: {e}", dir.display())
        }
    })?;
    let mut found: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_toml(path))
        .filter_map(|path| Some((path.file_stem()?.to_str()?.to_string(), path)))
        .collect();
    found.sort();
    Ok(found)
}

/// The file `wanted` names: a path when it contains `/` or ends in `.toml`, else a profile name
/// in `dir`. With nothing wanted, the directory's only profile, or a list of the choices.
///
/// # Errors
/// No such profile, no profiles at all, or several and no choice made.
pub fn resolve(dir: &Path, wanted: Option<&str>) -> Result<PathBuf, String> {
    if let Some(w) = wanted
        && (w.contains('/') || is_toml(Path::new(w)))
    {
        return Ok(PathBuf::from(w));
    }
    let all = list(dir)?;
    let names = || {
        all.iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match (wanted, all.as_slice()) {
        (Some(w), _) => all
            .iter()
            .find(|(n, _)| n == w)
            .map(|(_, p)| p.clone())
            .ok_or_else(|| format!("no profile {w:?} in {} (have: {})", dir.display(), names())),
        (None, [(_, only)]) => Ok(only.clone()),
        (None, []) => Err(format!(
            "no profiles in {}: run ./install.sh",
            dir.display()
        )),
        (None, _) => Err(format!(
            "several profiles in {}; pick one with --profile ({})",
            dir.display(),
            names()
        )),
    }
}

/// Every profile in `dir`, parsed. Two profiles for the same camera are refused: which one the
/// agent applied would depend on directory order.
///
/// # Errors
/// The directory cannot be read, a profile does not parse, or two claim the same camera.
pub fn load_all(dir: &Path) -> Result<Vec<(String, Profile)>, String> {
    let mut loaded: Vec<(String, Profile)> = Vec::new();
    for (name, path) in list(dir)? {
        let profile = Profile::load(&path)?;
        if let Some((other, _)) = loaded.iter().find(|(_, p)| p.device == profile.device) {
            return Err(format!(
                "profiles {other} and {name} both claim camera {:04x}:{:04x}",
                profile.device.vendor_id, profile.device.product_id
            ));
        }
        loaded.push((name, profile));
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::SHIPPED;

    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn dir_with(files: &[(&str, &str)]) -> PathBuf {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("webcam-profiles-{}-{n}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        dir
    }

    #[test]
    fn profiles_are_toml_files_named_by_their_stem() {
        let dir = dir_with(&[
            ("kiyo-pro.toml", SHIPPED),
            ("notes.txt", "x"),
            ("brio.toml", SHIPPED),
        ]);
        let names: Vec<String> = list(&dir).unwrap().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["brio", "kiyo-pro"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_takes_a_name_a_path_or_the_only_profile() {
        let dir = dir_with(&[("kiyo-pro.toml", SHIPPED)]);
        assert_eq!(resolve(&dir, None).unwrap(), dir.join("kiyo-pro.toml"));
        assert_eq!(
            resolve(&dir, Some("kiyo-pro")).unwrap(),
            dir.join("kiyo-pro.toml")
        );
        assert_eq!(
            resolve(&dir, Some("/x/y.toml")).unwrap(),
            PathBuf::from("/x/y.toml")
        );
        assert_eq!(
            resolve(&dir, Some("other.toml")).unwrap(),
            PathBuf::from("other.toml")
        );
        assert!(
            resolve(&dir, Some("brio"))
                .unwrap_err()
                .contains("(have: kiyo-pro)")
        );
        std::fs::write(dir.join("brio.toml"), SHIPPED).unwrap();
        assert!(
            resolve(&dir, None)
                .unwrap_err()
                .contains("pick one with --profile (brio, kiyo-pro)")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_or_missing_directory_says_how_to_fill_it() {
        let dir = dir_with(&[]);
        assert!(resolve(&dir, None).unwrap_err().contains("no profiles in"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(list(&dir).unwrap_err().contains("run ./install.sh"));
        assert!(
            list(Path::new("/etc/hosts")).is_err(),
            "a file is not a profile directory"
        );
    }

    #[test]
    fn two_profiles_for_one_camera_are_refused_and_distinct_ones_load() {
        let dir = dir_with(&[("a.toml", SHIPPED), ("b.toml", SHIPPED)]);
        assert!(
            load_all(&dir)
                .unwrap_err()
                .contains("profiles a and b both claim camera 1532:0e05")
        );
        std::fs::write(
            dir.join("b.toml"),
            SHIPPED.replace("product-id = 0x0E05", "product-id = 0x0E08"),
        )
        .unwrap();
        let loaded = load_all(&dir).unwrap();
        assert_eq!(
            loaded.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        std::fs::write(dir.join("b.toml"), "broken").unwrap();
        assert!(load_all(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(default_dir(None, Path::new("/h")).ends_with(".config/webcam_tune/profiles"));
        assert_eq!(
            default_dir(Some(Path::new("/x")), Path::new("/h")),
            Path::new("/x/webcam_tune/profiles")
        );
    }
}
