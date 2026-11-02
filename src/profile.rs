//! One profile file: which camera, how the agent behaves for it, and what it should hold.
//!
//! Every value lives in the file and nowhere in the binary: there is no compiled-in profile and
//! no default timing. A missing key is an error that names it, so a profile that parses is a
//! profile that says everything. The repo's `profiles/` are the starting points the installer
//! copies into place; the code only knows the UVC protocol (the `CONTROLS` table).

use std::path::Path;
use std::time::Duration;

use crate::settings::Settings;

/// The repo's Kiyo Pro profile, for the tests that hold it valid. The binary never reads it.
#[cfg(test)]
pub const SHIPPED: &str = include_str!("../profiles/kiyo-pro.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub vendor_id: u16,
    pub product_id: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agent {
    /// Gather a burst of plug-in and wake events this long, then apply once.
    pub settle: Duration,
    /// How long a camera that has just appeared gets to start answering.
    pub ready_timeout: Duration,
    /// How often to retry within that window.
    pub retry: Duration,
    /// The deadline for one USB control request.
    pub usb_timeout: Duration,
}

/// A profile's own version, `MAJOR.MINOR.PATCH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// # Errors
    /// Not three dot-separated whole numbers.
    pub fn parse(text: &str) -> Result<Self, String> {
        let parts: Vec<Result<u32, _>> = text.split('.').map(str::parse).collect();
        match parts.as_slice() {
            [Ok(major), Ok(minor), Ok(patch)] => Ok(Self(*major, *minor, *patch)),
            _ => Err(format!("version = {text:?} must be MAJOR.MINOR.PATCH")),
        }
    }

    #[must_use]
    pub const fn next_patch(self) -> Self {
        Self(self.0, self.1, self.2 + 1)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// What the profile is for, shown in every log line about it.
    pub name: String,
    pub description: String,
    pub version: Version,
    /// The day the profile last changed, `YYYY-MM-DD` (a TOML date in the file).
    pub updated: String,
    pub device: Device,
    pub agent: Agent,
    pub controls: Settings,
}

fn table<'a>(doc: &'a toml::Table, name: &str) -> Result<&'a toml::Table, String> {
    match doc.get(name) {
        Some(toml::Value::Table(t)) => Ok(t),
        Some(_) => Err(format!("[{name}] must be a table")),
        None => Err(format!("missing [{name}] table")),
    }
}

fn string(doc: &toml::Table, key: &str) -> Result<String, String> {
    match doc.get(key) {
        Some(toml::Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(toml::Value::String(_)) => Err(format!("{key} must not be empty")),
        Some(other) => Err(format!("{key} = {other} must be a string")),
        None => Err(format!("missing {key}")),
    }
}

fn date(doc: &toml::Table, key: &str) -> Result<String, String> {
    match doc.get(key) {
        Some(toml::Value::Datetime(dt))
            if dt.time.is_none() && dt.offset.is_none() && dt.date.is_some() =>
        {
            Ok(dt.to_string())
        }
        Some(other) => Err(format!("{key} = {other} must be a date, like 2026-11-01")),
        None => Err(format!("missing {key}")),
    }
}

fn integer(section: &toml::Table, name: &str, key: &str) -> Result<i64, String> {
    match section.get(key) {
        Some(toml::Value::Integer(n)) => Ok(*n),
        Some(other) => Err(format!("{name}.{key} = {other} must be an integer")),
        None => Err(format!("missing {name}.{key}")),
    }
}

fn u16_key(section: &toml::Table, name: &str, key: &str) -> Result<u16, String> {
    let n = integer(section, name, key)?;
    u16::try_from(n).map_err(|_| format!("{name}.{key} = {n} is not a USB id (0..=0xFFFF)"))
}

fn millis(section: &toml::Table, name: &str, key: &str) -> Result<Duration, String> {
    let n = integer(section, name, key)?;
    match u64::try_from(n) {
        Ok(ms) if ms > 0 => Ok(Duration::from_millis(ms)),
        _ => Err(format!(
            "{name}.{key} = {n} must be a positive number of milliseconds"
        )),
    }
}

fn refuse_unknown(section: &toml::Table, name: &str, known: &[&str]) -> Result<(), String> {
    section
        .keys()
        .find(|k| !known.contains(&k.as_str()))
        .map_or(Ok(()), |k| {
            Err(format!(
                "{name}.{k} is not a setting (known: {})",
                known.join(", ")
            ))
        })
}

const TOP_LEVEL: [&str; 7] = [
    "name",
    "description",
    "version",
    "updated",
    "device",
    "agent",
    "controls",
];
const DEVICE_KEYS: [&str; 2] = ["vendor-id", "product-id"];
const AGENT_KEYS: [&str; 4] = [
    "settle-ms",
    "ready-timeout-ms",
    "retry-ms",
    "usb-timeout-ms",
];

impl Profile {
    /// # Errors
    /// Not TOML, a missing or unknown section or key, or a value out of range; the message names it.
    pub fn parse(text: &str) -> Result<Self, String> {
        let doc: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| e.message().trim().to_string())?;
        refuse_unknown(&doc, "profile", &TOP_LEVEL)?;
        let device = table(&doc, "device")?;
        refuse_unknown(device, "device", &DEVICE_KEYS)?;
        let agent = table(&doc, "agent")?;
        refuse_unknown(agent, "agent", &AGENT_KEYS)?;
        Ok(Self {
            name: string(&doc, "name")?,
            description: string(&doc, "description")?,
            version: Version::parse(&string(&doc, "version")?)?,
            updated: date(&doc, "updated")?,
            device: Device {
                vendor_id: u16_key(device, "device", "vendor-id")?,
                product_id: u16_key(device, "device", "product-id")?,
            },
            agent: Agent {
                settle: millis(agent, "agent", "settle-ms")?,
                ready_timeout: millis(agent, "agent", "ready-timeout-ms")?,
                retry: millis(agent, "agent", "retry-ms")?,
                usb_timeout: millis(agent, "agent", "usb-timeout-ms")?,
            },
            controls: Settings::from_controls(table(&doc, "controls")?)?,
        })
    }

    /// # Errors
    /// The file cannot be read (a missing file says how to create one) or does not parse.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "no profile at {}: run ./install.sh, or copy one from the repo's profiles/",
                    path.display()
                )
            } else {
                format!("{}: {e}", path.display())
            }
        })?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The whole file, `header` as its leading comment.
    #[must_use]
    pub fn render(&self, header: &str) -> String {
        let comment: String = header
            .lines()
            .map(|l| {
                if l.is_empty() {
                    "#\n".to_string()
                } else {
                    format!("# {l}\n")
                }
            })
            .collect();
        let ms = |d: Duration| d.as_millis();
        let quoted = |s: &str| toml::Value::String(s.to_string()).to_string();
        format!(
            "{comment}\nname = {}\ndescription = {}\nversion = \"{}\"\nupdated = {}\n\n[device]\nvendor-id = 0x{:04X}  # USB vendor id\nproduct-id = 0x{:04X}  # USB product id\n\n\
             [agent]\nsettle-ms = {}  # gather a burst of plug-in and wake events this long, then apply once\n\
             ready-timeout-ms = {}  # how long a camera that has just appeared gets to start answering\n\
             retry-ms = {}  # how often to retry within that window\n\
             usb-timeout-ms = {}  # the deadline for one USB control request\n\n{}",
            quoted(&self.name),
            quoted(&self.description),
            self.version,
            self.updated,
            self.device.vendor_id,
            self.device.product_id,
            ms(self.agent.settle),
            ms(self.agent.ready_timeout),
            ms(self.agent.retry),
            ms(self.agent.usb_timeout),
            self.controls.render()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Value;

    fn full() -> String {
        Profile::parse(SHIPPED).unwrap().render("")
    }

    #[test]
    fn the_shipped_kiyo_pro_profile_says_everything_and_round_trips() {
        let profile = Profile::parse(SHIPPED).unwrap();
        assert_eq!(profile.name, "Razer Kiyo Pro (v1)");
        assert_eq!(profile.version, Version(1, 0, 0));
        assert_eq!(profile.updated, "2026-11-01");
        assert_eq!(
            profile.device,
            Device {
                vendor_id: 0x1532,
                product_id: 0x0E05
            }
        );
        assert_eq!(profile.agent.settle, Duration::from_millis(1500));
        assert_eq!(
            profile.controls.get("power-line-frequency"),
            Some(Value::Int(2))
        );
        assert_eq!(Profile::parse(&profile.render("again")).unwrap(), profile);
    }

    #[test]
    fn every_missing_key_is_named() {
        let full = full();
        let keys = ["name", "description", "version", "updated"]
            .iter()
            .chain(&DEVICE_KEYS)
            .chain(&AGENT_KEYS);
        for key in keys {
            let without: String = full
                .lines()
                .filter(|l| !l.starts_with(&format!("{key} =")))
                .flat_map(|l| [l, "\n"])
                .collect();
            let message = Profile::parse(&without).unwrap_err();
            assert!(
                message.starts_with("missing ") && message.ends_with(key),
                "{key}: {message}"
            );
        }
        for section in ["device", "agent", "controls"] {
            let renamed = full.replace(&format!("[{section}]"), "[elsewhere]");
            assert!(
                Profile::parse(&renamed).is_err(),
                "[{section}] gone must not parse"
            );
        }
    }

    #[test]
    fn unknown_keys_and_bad_values_are_refused() {
        let full = full();
        let cases = [
            (full.replace("[agent]\n", "[agent]\nsettle = 3\n"), "agent.settle is not a setting"),
            (full.replace("[device]\n", "[device]\nserial = 1\n"), "device.serial is not a setting"),
            (format!("author = \"me\"\n{full}"), "profile.author is not a setting"),
            (full.replace("vendor-id = 0x1532", "vendor-id = 70000"), "is not a USB id"),
            (full.replace("retry-ms = 250", "retry-ms = 0"), "positive number of milliseconds"),
            (full.replace("retry-ms = 250", "retry-ms = \"fast\""), "must be an integer"),
            (full.replace("version = \"1.0.0\"", "version = \"1.0\""), "must be MAJOR.MINOR.PATCH"),
            (full.replace("version = \"1.0.0\"", "version = 1"), "must be a string"),
            (full.replace("name = \"Razer Kiyo Pro (v1)\"", "name = \" \""), "name must not be empty"),
            (full.replace("updated = 2026-11-01", "updated = \"yesterday\""), "must be a date"),
            (full.replace("updated = 2026-11-01", "updated = 2026-11-01T10:00:00Z"), "must be a date"),
            (
                "name = \"n\"\ndescription = \"d\"\nversion = \"1.0.0\"\nupdated = 2026-11-01\nagent = 1\n\
                 [device]\nvendor-id = 1\nproduct-id = 2\n[controls]\ngain = 1\n"
                    .to_string(),
                "[agent] must be a table",
            ),
            ("[device".to_string(), ""),
        ];
        for (text, needle) in cases {
            let message = Profile::parse(&text).unwrap_err();
            assert!(message.contains(needle), "{needle}: {message}");
        }
    }

    #[test]
    fn names_and_descriptions_with_quotes_survive_a_round_trip() {
        let mut profile = Profile::parse(SHIPPED).unwrap();
        profile.name = "Cam \"one\" \\ two".into();
        assert_eq!(Profile::parse(&profile.render("")).unwrap(), profile);
    }

    #[test]
    fn versions_parse_print_and_step() {
        assert_eq!(Version::parse("2.10.3").unwrap(), Version(2, 10, 3));
        assert_eq!(Version(1, 0, 9).next_patch().to_string(), "1.0.10");
        assert!(Version::parse("1.0.x").is_err());
        assert!(Version::parse("1.0.0.0").is_err());
    }

    #[test]
    fn a_missing_file_says_how_to_make_one() {
        let message = Profile::load(Path::new("/nonexistent/webcam/kiyo-pro.toml")).unwrap_err();
        assert!(message.contains("run ./install.sh"), "{message}");
        assert!(
            Profile::load(Path::new("/"))
                .unwrap_err()
                .starts_with("/: ")
        );
    }
}
