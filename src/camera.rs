//! The camera: find the camera, locate its UVC units, read and write controls over USB.

use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use nusb::MaybeFuture;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};

use crate::controls::{self, Control, Unit, Value};
use crate::profile::{Agent, Device};

const SET_CUR: u8 = 0x01;
const GET_CUR: u8 = 0x81;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the rest of the program needs from a camera; the tests supply a fake.
pub trait Uvc {
    /// # Errors
    /// The camera refused the request, or answered with the wrong width.
    fn read(&self, control: &Control) -> Result<Value, Error>;
    /// # Errors
    /// The camera refused the value (for a manual value, usually because its auto mode is on).
    fn write(&self, control: &Control, value: Value) -> Result<(), Error>;
}

/// The video-control interface and the two units the controls are addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Units {
    pub interface: u8,
    pub camera: u8,
    pub processing: u8,
}

/// Read the unit ids out of the video-control interface's class-specific descriptors:
/// the camera terminal is the input terminal of type 0x0201, the processing unit is subtype 5.
#[must_use]
pub fn parse_units<'a>(
    interface: u8,
    descriptors: impl IntoIterator<Item = &'a [u8]>,
) -> Option<Units> {
    let (mut camera, mut processing) = (None, None);
    for d in descriptors {
        if d.len() < 4 || d[1] != 0x24 {
            continue;
        }
        match d[2] {
            0x02 if d.len() >= 6 && u16::from_le_bytes([d[4], d[5]]) == 0x0201 => {
                camera = Some(d[3]);
            }
            0x05 => processing = Some(d[3]),
            _ => {}
        }
    }
    Some(Units {
        interface,
        camera: camera?,
        processing: processing?,
    })
}

pub struct UvcCamera {
    device: nusb::Device,
    units: Units,
    usb_timeout: Duration,
}

impl UvcCamera {
    /// # Errors
    /// The camera is not attached, or it cannot be opened, or its descriptors name no camera units.
    pub fn open(wanted: Device, usb_timeout: Duration) -> Result<Self, Error> {
        let fail = |what: &str, e: &dyn fmt::Display| Error(format!("{what}: {e}"));
        let info = nusb::list_devices()
            .wait()
            .map_err(|e| fail("listing USB devices", &e))?
            .find(|d| d.vendor_id() == wanted.vendor_id && d.product_id() == wanted.product_id)
            .ok_or_else(|| {
                Error(format!(
                    "no camera {:04x}:{:04x} is attached",
                    wanted.vendor_id, wanted.product_id
                ))
            })?;
        let device = info
            .open()
            .wait()
            .map_err(|e| fail("opening the camera", &e))?;
        let config = device
            .active_configuration()
            .map_err(|e| fail("reading its configuration", &e))?;
        let units = config
            .interface_alt_settings()
            .filter(|alt| alt.class() == 0x0E && alt.subclass() == 0x01)
            .find_map(|alt| {
                let descriptors: Vec<Vec<u8>> = alt.descriptors().map(|d| d.to_vec()).collect();
                parse_units(
                    alt.interface_number(),
                    descriptors.iter().map(Vec::as_slice),
                )
            })
            .ok_or_else(|| {
                Error("the camera describes no UVC camera terminal and processing unit".into())
            })?;
        Ok(Self {
            device,
            units,
            usb_timeout,
        })
    }

    /// Whether the camera is on the bus now (no waiting, no opening).
    #[must_use]
    pub fn present(wanted: Device) -> bool {
        nusb::list_devices().wait().is_ok_and(|mut all| {
            all.any(|d| d.vendor_id() == wanted.vendor_id && d.product_id() == wanted.product_id)
        })
    }

    /// A camera that has just enumerated answers a moment later; retry, bounded by the agent's
    /// ready timeout.
    ///
    /// # Errors
    /// The last error seen when the deadline passes.
    pub fn open_within(wanted: Device, agent: &Agent) -> Result<Self, Error> {
        let start = Instant::now();
        loop {
            match Self::open(wanted, agent.usb_timeout)
                .and_then(|camera| camera.read(&controls::CONTROLS[0]).map(|_| camera))
            {
                Ok(camera) => return Ok(camera),
                Err(e) if start.elapsed() >= agent.ready_timeout => return Err(e),
                Err(_) => thread::sleep(agent.retry),
            }
        }
    }

    const fn address(&self, control: &Control) -> (u16, u16) {
        let unit = match control.unit {
            Unit::Camera => self.units.camera,
            Unit::Processing => self.units.processing,
        };
        (
            (control.selector as u16) << 8,
            ((unit as u16) << 8) | self.units.interface as u16,
        )
    }
}

impl Uvc for UvcCamera {
    fn read(&self, control: &Control) -> Result<Value, Error> {
        let (value, index) = self.address(control);
        let request = ControlIn {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: GET_CUR,
            value,
            index,
            length: control.kind.width(),
        };
        let bytes = self
            .device
            .control_in(request, self.usb_timeout)
            .wait()
            .map_err(|e| Error(format!("{}: read failed: {e}", control.name)))?;
        controls::decode(control.kind, &bytes).map_err(|e| Error(format!("{}: {e}", control.name)))
    }

    fn write(&self, control: &Control, value: Value) -> Result<(), Error> {
        let data = controls::encode(control.kind, value)
            .map_err(|e| Error(format!("{}: {e}", control.name)))?;
        let (selector, index) = self.address(control);
        let request = ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: SET_CUR,
            value: selector,
            index,
            data: &data,
        };
        self.device
            .control_out(request, self.usb_timeout)
            .wait()
            .map_err(|e| Error(format!("{}: the camera refused {value}: {e}", control.name)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The Kiyo Pro's video-control descriptors as the probe read them (2026-11-01), heads only
    // where the tail does not matter to the parser.
    const HEADER: &[u8] = &[
        0x0d, 0x24, 0x01, 0x00, 0x01, 0x69, 0x00, 0x80, 0xc3, 0xc9, 0x01, 0x01, 0x01,
    ];
    const OUTPUT_TERMINAL: &[u8] = &[0x09, 0x24, 0x03, 0x04, 0x01, 0x01, 0x00, 0x02, 0x00];
    const EXTENSION_UNIT: &[u8] = &[0x1b, 0x24, 0x06, 0x02, 0x6a, 0xd1, 0x49, 0x2c];
    const CAMERA_TERMINAL: &[u8] = &[
        0x12, 0x24, 0x02, 0x01, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    const PROCESSING_UNIT: &[u8] = &[
        0x0b, 0x24, 0x05, 0x03, 0x01, 0x00, 0x00, 0x02, 0x5b, 0x17, 0x00,
    ];

    #[test]
    fn the_kiyo_pro_descriptors_give_camera_terminal_1_and_processing_unit_3() {
        let all = [
            HEADER,
            OUTPUT_TERMINAL,
            EXTENSION_UNIT,
            CAMERA_TERMINAL,
            PROCESSING_UNIT,
        ];
        assert_eq!(
            parse_units(0, all),
            Some(Units {
                interface: 0,
                camera: 1,
                processing: 3
            })
        );
    }

    #[test]
    fn an_input_terminal_that_is_not_a_camera_is_not_taken_for_one() {
        let usb_streaming_terminal: &[u8] = &[0x08, 0x24, 0x02, 0x07, 0x01, 0x01, 0x00, 0x00];
        assert_eq!(
            parse_units(0, [usb_streaming_terminal, PROCESSING_UNIT]),
            None
        );
        assert_eq!(parse_units(0, [CAMERA_TERMINAL]), None);
        assert_eq!(
            parse_units(0, [&[0x24u8][..], &[0x04, 0x05, 0x05, 0x09][..]]),
            None
        );
    }

    #[test]
    fn errors_print_their_message() {
        assert_eq!(Error("no camera".into()).to_string(), "no camera");
    }
}
