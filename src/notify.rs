//! The kernel notifications the agent sleeps on, all on ONE run loop.
//!
//! Each profile's camera appearing on USB, and the Mac waking. This is the one place the crate
//! needs `unsafe` (`tests/unsafe_scope.rs` holds it here).
//!
//! Load-bearing rules from `IOKit`'s contract:
//!   * a matching notification is armed only once its iterator has been drained, and must be
//!     drained again on every callback, or it never fires again;
//!   * a client registered for system power MUST answer "can I sleep" and "I will sleep" with
//!     `IOAllowPowerChange`, or the Mac waits 30 s for it before every sleep;
//!   * "has powered on" is the message that means the Mac is awake again.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;

use core_foundation::base::TCFType;
use core_foundation::dictionary::CFMutableDictionary;
use core_foundation::number::CFNumber;
use core_foundation::runloop::{CFRunLoop, CFRunLoopSource, kCFRunLoopDefaultMode};
use core_foundation::string::CFString;
use io_kit_sys::keys::kIOFirstMatchNotification;
use io_kit_sys::ret::IOReturn;
use io_kit_sys::types::{io_connect_t, io_iterator_t, io_object_t, io_service_t};
use io_kit_sys::{
    IOIteratorNext, IONotificationPortCreate, IONotificationPortGetRunLoopSource,
    IONotificationPortRef, IOObjectRelease, IOServiceAddMatchingNotification, IOServiceMatching,
};

use crate::profile::Device;
use crate::watch::Event;

// iokit_common_msg(n) = sys_iokit | sub_iokit_common | n = 0xE000_0000 | n (IOMessage.h)
const CAN_SYSTEM_SLEEP: u32 = 0xE000_0270;
const SYSTEM_WILL_SLEEP: u32 = 0xE000_0280;
const SYSTEM_HAS_POWERED_ON: u32 = 0xE000_0300;

type InterestCallback = unsafe extern "C" fn(*mut c_void, io_service_t, u32, *mut c_void);

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IORegisterForSystemPower(
        refcon: *mut c_void,
        port: *mut IONotificationPortRef,
        callback: InterestCallback,
        notifier: *mut io_object_t,
    ) -> io_connect_t;
    fn IOAllowPowerChange(kernel_port: io_connect_t, notification_id: isize) -> IOReturn;
}

// One agent per process; the callbacks reach it here. A device callback's refcon is only an
// index into DEVICES, never a pointer to dereference.
static EVENTS: OnceLock<Sender<Event>> = OnceLock::new();
static DEVICES: OnceLock<Vec<Device>> = OnceLock::new();
static ROOT_PORT: AtomicU32 = AtomicU32::new(0);

/// What a power message asks of us: answer the kernel, report a wake, or nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    AllowSleep,
    ReportWake,
    Ignore,
}

#[must_use]
pub const fn action(message: u32) -> Action {
    match message {
        CAN_SYSTEM_SLEEP | SYSTEM_WILL_SLEEP => Action::AllowSleep,
        SYSTEM_HAS_POWERED_ON => Action::ReportWake,
        _ => Action::Ignore,
    }
}

fn send(event: Event) {
    // A send fails only once the applier is gone, and then there is no one left to tell.
    if let Some(events) = EVENTS.get()
        && events.send(event).is_err()
    {
        CFRunLoop::get_current().stop();
    }
}

/// Release every service the iterator holds (which re-arms it); how many there were.
fn drain(iterator: io_iterator_t) -> usize {
    let mut count = 0;
    loop {
        // SAFETY: `iterator` is the live matching iterator IOKit handed us.
        let service = unsafe { IOIteratorNext(iterator) };
        if service == 0 {
            return count;
        }
        // SAFETY: IOIteratorNext returned a retained object that we own and release once.
        unsafe { IOObjectRelease(service) };
        count += 1;
    }
}

unsafe extern "C" fn on_match(refcon: *mut c_void, iterator: io_iterator_t) {
    if drain(iterator) > 0
        && let Some(device) = DEVICES.get().and_then(|d| d.get(refcon.addr()))
    {
        send(Event::Plugged(*device));
    }
}

unsafe extern "C" fn on_power(
    _refcon: *mut c_void,
    _service: io_service_t,
    message: u32,
    argument: *mut c_void,
) {
    match action(message) {
        Action::AllowSleep => {
            // SAFETY: ROOT_PORT holds the connection IORegisterForSystemPower returned, and
            // `argument` is the notification id IOKit passed with this very message.
            let _: IOReturn =
                unsafe { IOAllowPowerChange(ROOT_PORT.load(Ordering::Acquire), argument as isize) };
        }
        Action::ReportWake => send(Event::Woke),
        Action::Ignore => {}
    }
}

fn add_to_run_loop(port: IONotificationPortRef) {
    // SAFETY: `port` is a live notification port; the source it returns follows the get rule,
    // which wrap_under_get_rule retains. kCFRunLoopDefaultMode is an immutable CF constant.
    let source =
        unsafe { CFRunLoopSource::wrap_under_get_rule(IONotificationPortGetRunLoopSource(port)) };
    CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopDefaultMode });
}

/// Arm the "camera appeared" notification for `DEVICES[index]`; whether it is already attached.
fn watch_device(index: usize, device: Device) -> Result<bool, String> {
    // SAFETY: 0 is MACH_PORT_NULL, the documented default main port (kIOMasterPortDefault and
    // kIOMainPortDefault are both spellings of it; the first is deprecated).
    let port = unsafe { IONotificationPortCreate(0) };
    if port.is_null() {
        return Err("IONotificationPortCreate failed".into());
    }
    // SAFETY: the class name is a NUL-terminated literal; IOServiceMatching returns a +1 dictionary.
    let raw = unsafe { IOServiceMatching(c"IOUSBHostDevice".as_ptr()) };
    if raw.is_null() {
        return Err("IOServiceMatching(IOUSBHostDevice) failed".into());
    }
    // SAFETY: `raw` is the +1 dictionary just created; the wrapper takes that reference.
    let mut matching: CFMutableDictionary<CFString, CFNumber> =
        unsafe { CFMutableDictionary::wrap_under_create_rule(raw) };
    matching.set(
        CFString::new("idVendor"),
        CFNumber::from(i32::from(device.vendor_id)),
    );
    matching.set(
        CFString::new("idProduct"),
        CFNumber::from(i32::from(device.product_id)),
    );
    let mut iterator: io_iterator_t = 0;
    let dictionary = matching.as_concrete_TypeRef();
    // IOServiceAddMatchingNotification consumes one reference: hand it ours.
    std::mem::forget(matching);
    // SAFETY: every pointer is valid for the call; the callback has the exact C signature.
    let status = unsafe {
        IOServiceAddMatchingNotification(
            port,
            kIOFirstMatchNotification,
            dictionary.cast(),
            on_match,
            std::ptr::without_provenance_mut(index),
            &raw mut iterator,
        )
    };
    if status != 0 {
        return Err(format!(
            "IOServiceAddMatchingNotification failed: 0x{status:08x}"
        ));
    }
    add_to_run_loop(port);
    Ok(drain(iterator) > 0)
}

fn watch_power() -> Result<(), String> {
    let mut port: IONotificationPortRef = std::ptr::null_mut();
    let mut notifier: io_object_t = 0;
    // SAFETY: both out-pointers are valid locals; the callback has the exact C signature.
    let root = unsafe {
        IORegisterForSystemPower(
            std::ptr::null_mut(),
            &raw mut port,
            on_power,
            &raw mut notifier,
        )
    };
    if root == 0 || port.is_null() {
        return Err("IORegisterForSystemPower refused the registration".into());
    }
    ROOT_PORT.store(root, Ordering::Release);
    add_to_run_loop(port);
    Ok(())
}

/// Register the wake notification and one plug-in notification per camera, then run this
/// thread's run loop forever. `Event::Started` is sent at once for each camera already attached.
///
/// # Errors
/// A registration failed, or a second agent was started in this process.
pub fn run(devices: Vec<Device>, events: Sender<Event>) -> Result<(), String> {
    EVENTS
        .set(events)
        .map_err(|_| "the agent is already running in this process".to_string())?;
    DEVICES
        .set(devices.clone())
        .map_err(|_| "the agent is already running in this process".to_string())?;
    watch_power()?;
    for (index, device) in devices.into_iter().enumerate() {
        if watch_device(index, device)? {
            send(Event::Started(device));
        }
    }
    CFRunLoop::run_current();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_requests_are_always_answered_and_only_power_on_counts_as_a_wake() {
        assert_eq!(action(CAN_SYSTEM_SLEEP), Action::AllowSleep);
        assert_eq!(action(SYSTEM_WILL_SLEEP), Action::AllowSleep);
        assert_eq!(action(SYSTEM_HAS_POWERED_ON), Action::ReportWake);
        // kIOMessageSystemWillPowerOn arrives before the hardware is back: too early to apply.
        assert_eq!(action(0xE000_0320), Action::Ignore);
    }
}
