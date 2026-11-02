//! The UVC controls this program knows: where each lives, how wide it is on the wire, what it means.
//!
//! Selectors are the UVC 1.1 ones (the camera reports bcdUVC 1.00 and implements exactly the
//! sixteen below; `uvc-util -c` lists the same set). The table is in APPLY order: an auto mode
//! precedes the manual value it governs, because the camera refuses a manual value while its
//! auto mode is on (measured: exit 13 from uvc-util for exposure, white balance and focus).

use std::fmt;

/// Which UVC entity a control is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Camera,
    Processing,
}

/// The control's wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    U8,
    U16,
    I16,
    U32,
    Bool,
    PanTilt,
}

impl Kind {
    #[must_use]
    pub const fn width(self) -> u16 {
        match self {
            Self::U8 | Self::Bool => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 => 4,
            Self::PanTilt => 8,
        }
    }
}

/// A control's value as the profile and the camera both speak it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    PanTilt { pan: i32, tilt: i32 },
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::PanTilt { pan, tilt } => write!(f, "{{ pan = {pan}, tilt = {tilt} }}"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Control {
    pub name: &'static str,
    pub unit: Unit,
    pub selector: u8,
    pub kind: Kind,
    pub about: &'static str,
}

const fn c(
    name: &'static str,
    unit: Unit,
    selector: u8,
    kind: Kind,
    about: &'static str,
) -> Control {
    Control {
        name,
        unit,
        selector,
        kind,
        about,
    }
}

pub static CONTROLS: [Control; 16] = [
    c(
        "power-line-frequency",
        Unit::Processing,
        0x05,
        Kind::U8,
        "anti-flicker: 1 = 50 Hz mains, 2 = 60 Hz (North America)",
    ),
    c(
        "backlight-compensation",
        Unit::Processing,
        0x01,
        Kind::U16,
        "\"HDR\": 1 lifts shadows, caps the camera at 30 fps, flattens the image",
    ),
    c(
        "auto-exposure-mode",
        Unit::Camera,
        0x02,
        Kind::U8,
        "8 = automatic exposure, 1 = manual (exposure-time-abs applies)",
    ),
    c(
        "auto-exposure-priority",
        Unit::Camera,
        0x03,
        Kind::U8,
        "0 = hold the frame rate in dim light, 1 = slow it down to expose",
    ),
    c(
        "auto-white-balance-temp",
        Unit::Processing,
        0x0B,
        Kind::Bool,
        "true = automatic white balance, false = white-balance-temp applies",
    ),
    c(
        "auto-focus",
        Unit::Camera,
        0x08,
        Kind::Bool,
        "true = autofocus, false = focus-abs applies (no hunting)",
    ),
    c(
        "exposure-time-abs",
        Unit::Camera,
        0x04,
        Kind::U32,
        "exposure in 100 us units, 3..2047 (manual exposure only)",
    ),
    c(
        "gain",
        Unit::Processing,
        0x04,
        Kind::U16,
        "sensor gain, 0..255",
    ),
    c(
        "white-balance-temp",
        Unit::Processing,
        0x0A,
        Kind::U16,
        "kelvin, 2000..7500 in steps of 10 (manual white balance only)",
    ),
    c(
        "focus-abs",
        Unit::Camera,
        0x06,
        Kind::U16,
        "lens position, 0..600 (manual focus only)",
    ),
    c(
        "brightness",
        Unit::Processing,
        0x02,
        Kind::I16,
        "0..255, factory 128",
    ),
    c(
        "contrast",
        Unit::Processing,
        0x03,
        Kind::U16,
        "0..255, factory 128",
    ),
    c(
        "saturation",
        Unit::Processing,
        0x07,
        Kind::U16,
        "0..255, factory 128",
    ),
    c(
        "sharpness",
        Unit::Processing,
        0x08,
        Kind::U16,
        "0..255, factory 128; lower it if edges halo",
    ),
    c(
        "zoom-abs",
        Unit::Camera,
        0x0B,
        Kind::U16,
        "100..400, 100 = widest",
    ),
    c(
        "pan-tilt-abs",
        Unit::Camera,
        0x0D,
        Kind::PanTilt,
        "arc-seconds, steps of 3600; only moves the picture when zoomed in",
    ),
];

/// manual control, its auto mode, and the value that mode must hold for the manual one to apply
pub static GOVERNED: [(&str, &str, Value); 3] = [
    ("exposure-time-abs", "auto-exposure-mode", Value::Int(1)),
    (
        "white-balance-temp",
        "auto-white-balance-temp",
        Value::Bool(false),
    ),
    ("focus-abs", "auto-focus", Value::Bool(false)),
];

#[must_use]
pub fn find(name: &str) -> Option<&'static Control> {
    CONTROLS.iter().find(|control| control.name == name)
}

/// # Errors
/// The value is the wrong type for the control, or does not fit its wire width.
pub fn encode(kind: Kind, value: Value) -> Result<Vec<u8>, String> {
    let fits = |n: i64, lo: i64, hi: i64| {
        if (lo..=hi).contains(&n) {
            Ok(n)
        } else {
            Err(format!("{n} is outside {lo}..={hi}"))
        }
    };
    match (kind, value) {
        (Kind::U8, Value::Int(n)) => Ok(vec![
            u8::try_from(fits(n, 0, 0xFF)?).map_err(|e| e.to_string())?,
        ]),
        (Kind::U16, Value::Int(n)) => Ok(u16::try_from(fits(n, 0, 0xFFFF)?)
            .map_err(|e| e.to_string())?
            .to_le_bytes()
            .to_vec()),
        (Kind::I16, Value::Int(n)) => Ok(i16::try_from(fits(n, -0x8000, 0x7FFF)?)
            .map_err(|e| e.to_string())?
            .to_le_bytes()
            .to_vec()),
        (Kind::U32, Value::Int(n)) => Ok(u32::try_from(fits(n, 0, 0xFFFF_FFFF)?)
            .map_err(|e| e.to_string())?
            .to_le_bytes()
            .to_vec()),
        (Kind::Bool, Value::Bool(b)) => Ok(vec![u8::from(b)]),
        (Kind::PanTilt, Value::PanTilt { pan, tilt }) => {
            Ok([pan.to_le_bytes(), tilt.to_le_bytes()].concat())
        }
        (kind, value) => Err(format!("{value} is not a {kind:?} value")),
    }
}

/// # Errors
/// The camera answered with a different number of bytes than the control is wide.
pub fn decode(kind: Kind, bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() != usize::from(kind.width()) {
        return Err(format!(
            "expected {} bytes, the camera sent {}",
            kind.width(),
            bytes.len()
        ));
    }
    let word = |at: usize| [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
    Ok(match kind {
        Kind::U8 => Value::Int(i64::from(bytes[0])),
        Kind::Bool => Value::Bool(bytes[0] != 0),
        Kind::U16 => Value::Int(i64::from(u16::from_le_bytes([bytes[0], bytes[1]]))),
        Kind::I16 => Value::Int(i64::from(i16::from_le_bytes([bytes[0], bytes[1]]))),
        Kind::U32 => Value::Int(i64::from(u32::from_le_bytes(word(0)))),
        Kind::PanTilt => Value::PanTilt {
            pan: i32::from_le_bytes(word(0)),
            tilt: i32::from_le_bytes(word(4)),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_round_trips_through_its_wire_bytes() {
        let cases = [
            (Kind::U8, Value::Int(8)),
            (Kind::U16, Value::Int(5137)),
            (Kind::I16, Value::Int(-3)),
            (Kind::U32, Value::Int(1734)),
            (Kind::Bool, Value::Bool(true)),
            (
                Kind::PanTilt,
                Value::PanTilt {
                    pan: -3600,
                    tilt: 7200,
                },
            ),
        ];
        for (kind, value) in cases {
            let bytes = encode(kind, value).unwrap();
            assert_eq!(bytes.len(), usize::from(kind.width()), "{kind:?}");
            assert_eq!(decode(kind, &bytes).unwrap(), value, "{kind:?}");
        }
    }

    #[test]
    fn brightness_is_the_little_endian_word_the_camera_sent() {
        // The probe read brightness as [33, 0] and uvc-util printed 33.
        assert_eq!(decode(Kind::I16, &[33, 0]).unwrap(), Value::Int(33));
    }

    #[test]
    fn a_value_that_does_not_fit_or_is_the_wrong_type_is_refused() {
        assert!(
            encode(Kind::U8, Value::Int(256))
                .unwrap_err()
                .contains("outside")
        );
        assert!(
            encode(Kind::U16, Value::Int(-1))
                .unwrap_err()
                .contains("outside")
        );
        assert!(
            encode(Kind::Bool, Value::Int(1))
                .unwrap_err()
                .contains("not a Bool")
        );
        assert!(
            decode(Kind::U16, &[1])
                .unwrap_err()
                .contains("expected 2 bytes")
        );
    }

    #[test]
    fn the_table_names_each_control_once_and_every_governor_precedes_what_it_governs() {
        for (i, control) in CONTROLS.iter().enumerate() {
            assert_eq!(
                CONTROLS.iter().position(|c| c.name == control.name),
                Some(i),
                "{}",
                control.name
            );
        }
        let at = |name: &str| CONTROLS.iter().position(|c| c.name == name).unwrap();
        for (manual, auto, _) in &GOVERNED {
            assert!(
                at(auto) < at(manual),
                "{auto} must be written before {manual}"
            );
        }
    }

    #[test]
    fn values_print_as_the_profile_spells_them() {
        assert_eq!(Value::Int(128).to_string(), "128");
        assert_eq!(Value::Bool(false).to_string(), "false");
        assert_eq!(
            Value::PanTilt { pan: 0, tilt: 0 }.to_string(),
            "{ pan = 0, tilt = 0 }"
        );
        assert_eq!(find("gain").map(|c| c.selector), Some(0x04));
        assert!(find("hue").is_none());
    }
}
