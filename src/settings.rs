//! A profile: the control values the camera should hold, and how to make it hold them.
//!
//! The ORDER is the camera's (the `CONTROLS` table), never the file's. A governed value whose
//! auto mode the profile leaves on is skipped and named rather than sent to be refused, and every
//! write is believed only once it reads back: the camera clamps an out-of-range value and reports
//! success (measured: brightness 999 reads back 255).

use crate::camera::{Error, Uvc};
use crate::controls::{CONTROLS, Control, GOVERNED, Kind, Value, find};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    values: Vec<(&'static Control, Value)>,
}

impl Settings {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Value> {
        self.values
            .iter()
            .find(|(c, _)| c.name == name)
            .map(|(_, v)| *v)
    }

    pub fn set(&mut self, control: &'static Control, value: Value) {
        if let Some(slot) = self.values.iter_mut().find(|(c, _)| c.name == control.name) {
            slot.1 = value;
        } else {
            self.values.push((control, value));
        }
        self.values
            .sort_by_key(|(c, _)| CONTROLS.iter().position(|k| k.name == c.name));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static Control, Value)> + '_ {
        self.values.iter().copied()
    }

    /// # Errors
    /// Not TOML, or no `[controls]` table, or any error of [`Self::from_controls`].
    pub fn parse(text: &str) -> Result<Self, String> {
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| e.message().to_string())?;
        match table.get("controls") {
            Some(toml::Value::Table(controls)) => Self::from_controls(controls),
            _ => Err("no [controls] table".into()),
        }
    }

    /// # Errors
    /// A name that is not a Kiyo Pro control, or a value of the wrong type. A typo must fail
    /// loudly: a misspelt control would otherwise never be applied.
    pub fn from_controls(controls: &toml::Table) -> Result<Self, String> {
        let mut profile = Self::default();
        for (name, raw) in controls {
            let control =
                find(name).ok_or_else(|| format!("{name:?} is not a Kiyo Pro control"))?;
            let value = from_toml(control.kind, raw)
                .ok_or_else(|| format!("{name} = {raw} is not a {:?} value", control.kind))?;
            profile.set(control, value);
        }
        Ok(profile)
    }

    /// The `[controls]` table, one commented line per control.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::from("[controls]\n");
        out.extend(
            self.values.iter().map(|(control, value)| {
                format!("{} = {value}  # {}\n", control.name, control.about)
            }),
        );
        out
    }
}

fn from_toml(kind: Kind, raw: &toml::Value) -> Option<Value> {
    match (kind, raw) {
        (Kind::Bool, toml::Value::Boolean(b)) => Some(Value::Bool(*b)),
        (Kind::PanTilt, toml::Value::Table(t)) => {
            let part = |k| {
                t.get(k)
                    .and_then(toml::Value::as_integer)
                    .and_then(|n| i32::try_from(n).ok())
            };
            Some(Value::PanTilt {
                pan: part("pan")?,
                tilt: part("tilt")?,
            })
        }
        (Kind::U8 | Kind::U16 | Kind::I16 | Kind::U32, toml::Value::Integer(n)) => {
            Some(Value::Int(*n))
        }
        _ => None,
    }
}

/// (writes in apply order, governed controls skipped with the reason).
#[must_use]
pub fn plan(profile: &Settings) -> (Vec<(&'static Control, Value)>, Vec<String>) {
    let (mut writes, mut skipped) = (Vec::new(), Vec::new());
    for (control, value) in profile.iter() {
        if let Some((_, auto, manual)) = GOVERNED.iter().find(|(m, _, _)| *m == control.name)
            && profile.get(auto) != Some(*manual)
        {
            skipped.push(format!(
                "{}: applies only with {auto} = {manual}",
                control.name
            ));
            continue;
        }
        writes.push((control, value));
    }
    (writes, skipped)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub applied: Vec<&'static str>,
    pub failed: Vec<String>,
    pub skipped: Vec<String>,
}

pub fn apply(camera: &dyn Uvc, profile: &Settings) -> Report {
    let (writes, skipped) = plan(profile);
    let mut report = Report {
        skipped,
        ..Report::default()
    };
    for (control, want) in writes {
        match camera
            .write(control, want)
            .and_then(|()| camera.read(control))
        {
            Ok(got) if got == want => report.applied.push(control.name),
            Ok(got) => report
                .failed
                .push(format!("{} = {want}: the camera holds {got}", control.name)),
            Err(e) => report.failed.push(e.0),
        }
    }
    report
}

/// What the camera holds now, minus the governed values its auto modes are overriding.
///
/// # Errors
/// Any control the camera will not read.
pub fn read(camera: &dyn Uvc) -> Result<Settings, Error> {
    let mut profile = Settings::default();
    for control in &CONTROLS {
        profile.set(control, camera.read(control)?);
    }
    for (manual, auto, needed) in &GOVERNED {
        if profile.get(auto) != Some(*needed) {
            profile.values.retain(|(c, _)| c.name != *manual);
        }
    }
    Ok(profile)
}

#[cfg(test)]
pub mod fake {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    /// A camera that behaves as the real one was measured to: it refuses a manual value while its
    /// auto mode is on, and clamps a 0..255 control's value above 255 instead of refusing it.
    pub struct Camera {
        pub state: RefCell<HashMap<&'static str, Value>>,
        pub broken: Option<&'static str>,
    }

    const BYTE_RANGED: [&str; 5] = ["brightness", "contrast", "saturation", "sharpness", "gain"];

    impl Default for Camera {
        fn default() -> Self {
            let mut state = HashMap::new();
            for control in &CONTROLS {
                let value = match control.kind {
                    Kind::Bool => Value::Bool(true),
                    Kind::PanTilt => Value::PanTilt { pan: 0, tilt: 0 },
                    _ => Value::Int(match control.name {
                        "auto-exposure-mode" => 8,
                        "power-line-frequency" => 2,
                        "zoom-abs" => 100,
                        _ => 128,
                    }),
                };
                state.insert(control.name, value);
            }
            Self {
                state: RefCell::new(state),
                broken: None,
            }
        }
    }

    impl Uvc for Camera {
        fn read(&self, control: &Control) -> Result<Value, Error> {
            if self.broken == Some(control.name) {
                return Err(Error(format!("{}: read failed", control.name)));
            }
            Ok(self.state.borrow()[control.name])
        }

        fn write(&self, control: &Control, value: Value) -> Result<(), Error> {
            if let Some((_, auto, manual)) = GOVERNED.iter().find(|(m, _, _)| *m == control.name)
                && self.state.borrow()[auto] != *manual
            {
                return Err(Error(format!(
                    "{}: the camera refused {value}",
                    control.name
                )));
            }
            let stored = match value {
                Value::Int(n) if BYTE_RANGED.contains(&control.name) => Value::Int(n.min(255)),
                other => other,
            };
            self.state.borrow_mut().insert(control.name, stored);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_default_profile_parses_and_applies_cleanly() {
        let profile = Settings::parse(crate::profile::SHIPPED).unwrap();
        let camera = fake::Camera::default();
        let report = apply(&camera, &profile);
        assert_eq!(report.failed, Vec::<String>::new());
        assert_eq!(
            report.applied.len(),
            profile.iter().count() - report.skipped.len()
        );
    }

    #[test]
    fn auto_modes_are_written_before_the_values_they_govern_whatever_the_file_order() {
        let profile = Settings::parse("[controls]\nfocus-abs = 300\nauto-focus = false\n").unwrap();
        let camera = fake::Camera::default();
        let report = apply(&camera, &profile);
        assert_eq!(report.applied, vec!["auto-focus", "focus-abs"]);
        assert_eq!(camera.state.borrow()["focus-abs"], Value::Int(300));
    }

    #[test]
    fn a_governed_value_with_its_auto_mode_left_on_is_skipped_and_named() {
        let profile =
            Settings::parse("[controls]\nwhite-balance-temp = 4500\nbrightness = 100\n").unwrap();
        let (writes, skipped) = plan(&profile);
        assert_eq!(
            writes.iter().map(|(c, _)| c.name).collect::<Vec<_>>(),
            vec!["brightness"]
        );
        assert_eq!(
            skipped,
            vec!["white-balance-temp: applies only with auto-white-balance-temp = false"]
        );
    }

    #[test]
    fn a_clamped_write_is_a_failure_not_a_success() {
        let profile = Settings::parse("[controls]\nbrightness = 300\n").unwrap();
        let report = apply(&fake::Camera::default(), &profile);
        assert_eq!(
            report.failed,
            vec!["brightness = 300: the camera holds 255"]
        );
        assert_eq!(report.applied, Vec::<&str>::new());
    }

    #[test]
    fn a_refused_or_unreadable_control_is_reported_and_the_rest_still_apply() {
        let camera = fake::Camera {
            broken: Some("contrast"),
            ..fake::Camera::default()
        };
        let profile = Settings::parse("[controls]\ncontrast = 100\nsharpness = 90\n").unwrap();
        let report = apply(&camera, &profile);
        assert_eq!(report.failed, vec!["contrast: read failed"]);
        assert_eq!(report.applied, vec!["sharpness"]);
        assert_eq!(read(&camera).unwrap_err().0, "contrast: read failed");
    }

    #[test]
    fn typos_and_wrong_types_fail_loudly() {
        assert!(
            Settings::parse("[controls]\nbrightnes = 1\n")
                .unwrap_err()
                .contains("not a Kiyo Pro control")
        );
        assert!(
            Settings::parse("[controls]\nauto-focus = 1\n")
                .unwrap_err()
                .contains("not a Bool value")
        );
        assert!(
            Settings::parse("[controls]\npan-tilt-abs = { pan = 0 }\n")
                .unwrap_err()
                .contains("PanTilt")
        );
        assert_eq!(
            Settings::parse("brightness = 1\n").unwrap_err(),
            "no [controls] table"
        );
        assert!(Settings::parse("[controls\n").is_err());
    }

    #[test]
    fn capture_drops_governed_values_the_auto_modes_override_and_round_trips_through_toml() {
        let camera = fake::Camera::default();
        let captured = read(&camera).unwrap();
        for name in ["exposure-time-abs", "white-balance-temp", "focus-abs"] {
            assert_eq!(
                captured.get(name),
                None,
                "{name} is overridden by its auto mode"
            );
        }
        let text = captured.render();
        assert!(text.starts_with("[controls]\n"));
        assert_eq!(Settings::parse(&text).unwrap(), captured);
    }

    #[test]
    fn a_manual_value_is_captured_once_its_auto_mode_is_off() {
        let camera = fake::Camera::default();
        camera
            .state
            .borrow_mut()
            .insert("auto-focus", Value::Bool(false));
        assert_eq!(
            read(&camera).unwrap().get("focus-abs"),
            Some(Value::Int(128))
        );
    }
}
