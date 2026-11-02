//! Re-apply the profile when the camera arrives and when the Mac wakes, and do nothing otherwise.
//!
//! Both sources are kernel notifications on one run loop (src/notify.rs), so the process sleeps
//! between them. Events arrive in bursts (a wake often re-enumerates the camera too), so they are
//! coalesced over the config's settle window and applied once.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use crate::profile::Device;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The camera was already attached when the agent started.
    Started(Device),
    Plugged(Device),
    Woke,
}

impl Event {
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Started(_) => "agent started",
            Self::Plugged(_) => "camera plugged in",
            Self::Woke => "Mac woke",
        }
    }
}

/// Everything that arrives within `settle` of the last event, after `first`: the burst's
/// distinct reasons in arrival order.
#[must_use]
pub fn gather(first: Event, events: &Receiver<Event>, settle: Duration) -> Vec<Event> {
    let mut burst = vec![first];
    loop {
        match events.recv_timeout(settle) {
            Ok(event) if !burst.contains(&event) => burst.push(event),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return burst,
        }
    }
}

/// Block for the next burst, or `None` once every sender is gone.
#[must_use]
pub fn next_burst(events: &Receiver<Event>, settle: Duration) -> Option<Vec<Event>> {
    events
        .recv()
        .ok()
        .map(|first| gather(first, events, settle))
}

/// The cameras a burst is about: each one that appeared, and, after a wake, every camera
/// (`None`), because the Mac does not say which devices lost their settings while it slept.
#[must_use]
pub fn targets(burst: &[Event]) -> Option<Vec<Device>> {
    if burst.contains(&Event::Woke) {
        return None;
    }
    let mut devices = Vec::new();
    for event in burst {
        if let Event::Started(d) | Event::Plugged(d) = event
            && !devices.contains(d)
        {
            devices.push(*d);
        }
    }
    Some(devices)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    const KIYO: Device = Device {
        vendor_id: 0x1532,
        product_id: 0x0E05,
    };
    const BRIO: Device = Device {
        vendor_id: 0x046D,
        product_id: 0x085E,
    };

    #[test]
    fn a_burst_is_coalesced_into_its_distinct_reasons_in_order() {
        let (tx, rx) = mpsc::channel();
        for event in [
            Event::Woke,
            Event::Plugged(KIYO),
            Event::Woke,
            Event::Plugged(KIYO),
        ] {
            tx.send(event).unwrap();
        }
        assert_eq!(
            next_burst(&rx, Duration::from_millis(20)),
            Some(vec![Event::Woke, Event::Plugged(KIYO)])
        );
    }

    #[test]
    fn events_after_the_settle_window_start_the_next_burst() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Started(KIYO)).unwrap();
        let late = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            tx.send(Event::Plugged(KIYO)).unwrap();
        });
        assert_eq!(
            next_burst(&rx, Duration::from_millis(20)),
            Some(vec![Event::Started(KIYO)])
        );
        assert_eq!(
            next_burst(&rx, Duration::from_millis(20)),
            Some(vec![Event::Plugged(KIYO)])
        );
        late.join().unwrap();
        assert_eq!(next_burst(&rx, Duration::from_millis(20)), None);
    }

    #[test]
    fn every_event_says_why_it_fired() {
        assert_eq!(Event::Started(KIYO).reason(), "agent started");
        assert_eq!(Event::Plugged(KIYO).reason(), "camera plugged in");
        assert_eq!(Event::Woke.reason(), "Mac woke");
    }

    #[test]
    fn a_plug_in_targets_its_camera_and_a_wake_targets_all() {
        let plugged = [
            Event::Plugged(KIYO),
            Event::Started(BRIO),
            Event::Plugged(KIYO),
        ];
        assert_eq!(targets(&plugged), Some(vec![KIYO, BRIO]));
        assert_eq!(targets(&[Event::Plugged(KIYO), Event::Woke]), None);
    }
}
