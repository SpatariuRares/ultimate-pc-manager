//! Sensor interests that depend on what is on screen, owned by the add-on
//! host so that no surface writes lifetime code for them.
//!
//! A [`Surface`] (an add-on's page, or the core Summary page) asks for sensors while it draws
//! ([`crate::platform::PageContext::sensors`]). The first ask creates the
//! surface's interest in the shared sensor service; every frame in which the
//! surface asks again keeps it; the host drops it at the end of the first
//! frame in which the surface did not ask ([`Leases::end_frame`]), and when
//! the add-on is turned off ([`Leases::release`]).

use crate::sensors::{Interest, SensorService};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

/// Something on screen whose sensor use lasts while it is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Surface {
    /// The page of the add-on with this id.
    Page(&'static str),
    /// The core Summary page: no add-on owns it, so only the end of a frame
    /// drops it.
    Summary,
}

impl Surface {
    /// The id of the add-on this surface belongs to, if one does.
    fn addon(self) -> Option<&'static str> {
        match self {
            Surface::Page(id) => Some(id),
            Surface::Summary => None,
        }
    }
}

/// What a surface reads sensors through while it draws: the latest frame,
/// history, statistics and "read now" of the shared service. Valid for the
/// draw it was given for: keep it no longer, the host owns the lifetime of
/// the interest behind it.
pub struct Sensors(Arc<Interest>);

impl Deref for Sensors {
    type Target = Interest;

    fn deref(&self) -> &Interest {
        &self.0
    }
}

/// A surface's interest and whether the surface asked in this frame.
struct Held {
    interest: Arc<Interest>,
    asked: bool,
}

/// The interests of the surfaces drawn now. Cheap to clone.
#[derive(Clone)]
pub struct Leases(Arc<Inner>);

struct Inner {
    service: SensorService,
    held: Mutex<HashMap<Surface, Held>>,
}

impl Leases {
    /// Leases over `service`; nothing runs until a surface asks.
    pub fn new(service: SensorService) -> Self {
        Leases(Arc::new(Inner { service, held: Mutex::default() }))
    }

    /// `surface` asks for sensors in this frame: its interest is created on
    /// the first ask and kept by every frame that asks again.
    pub fn ask(&self, surface: Surface) -> Sensors {
        let mut held = self.0.held.lock().unwrap_or_else(|e| e.into_inner());
        let entry = held.entry(surface).or_insert_with(|| Held { interest: Arc::new(self.0.service.interest()), asked: false });
        entry.asked = true;
        Sensors(entry.interest.clone())
    }

    /// Ends a frame: drops the interest of every surface that did not ask in
    /// it. The others keep theirs for the next frame.
    pub fn end_frame(&self) {
        let gone = self.take(|_, h| !h.asked);
        for h in self.0.held.lock().unwrap_or_else(|e| e.into_inner()).values_mut() {
            h.asked = false;
        }
        drop(gone);
    }

    /// Drops the interests of the add-on `id` (it was turned off).
    pub fn release(&self, id: &str) {
        drop(self.take(|s, _| s.addon() == Some(id)));
    }

    /// "Refresh" of the page of `id`: reads every sensor at once, when that
    /// page holds an interest.
    pub fn read_now(&self, surface: Surface) {
        let interest = self.0.held.lock().unwrap_or_else(|e| e.into_inner()).get(&surface).map(|h| h.interest.clone());
        if let Some(interest) = interest {
            interest.read_now();
        }
    }

    /// Removes the surfaces `pick` selects and returns their interests, to
    /// be dropped without the lock.
    fn take(&self, pick: impl Fn(Surface, &Held) -> bool) -> Vec<Held> {
        let mut held = self.0.held.lock().unwrap_or_else(|e| e.into_inner());
        let gone: Vec<Surface> = held.iter().filter(|(s, h)| pick(**s, h)).map(|(s, _)| *s).collect();
        gone.into_iter().filter_map(|s| held.remove(&s)).collect()
    }
}
