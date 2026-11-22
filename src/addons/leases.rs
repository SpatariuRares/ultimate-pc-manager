//! Sensor interests that depend on what is on screen, owned by the add-on
//! host so that no surface writes lifetime code for them.
//!
//! A [`Surface`] (an add-on's page or Summary tile, or the core Summary page) asks for sensors while it draws
//! ([`crate::platform::PageContext::sensors`]). The first ask creates the
//! surface's interest in the shared sensor service; every frame in which the
//! surface asks again keeps it; the host drops it at the end of the first
//! frame in which the surface did not ask ([`Leases::end_frame`]), and when
//! the add-on is turned off or stopped ([`Leases::release`]).

use crate::sensors::{Interest, SensorService};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard};

/// Something on screen whose sensor use lasts while it is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Surface {
    /// The page of the add-on with this id.
    Page(&'static str),
    /// The Summary tile of the add-on with this id.
    Tile(&'static str),
    /// The core Summary page: no add-on owns it, so only the end of a frame
    /// drops it.
    Summary,
}

impl Surface {
    /// The id of the add-on this surface belongs to, if one does.
    fn addon(self) -> Option<&'static str> {
        match self {
            Surface::Page(id) | Surface::Tile(id) => Some(id),
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

/// How a surface being drawn reaches its sensors: the host's [`Leases`] and
/// the [`Surface`] they are kept for. Set on the page context by the host.
#[derive(Clone)]
pub struct Lease {
    leases: Leases,
    surface: Surface,
}

impl Lease {
    /// The sensors of this surface, asked in this frame.
    pub fn ask(&self) -> Sensors {
        self.leases.ask(self.surface)
    }
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

    /// What `surface` reads its sensors through while it draws.
    pub fn lease(&self, surface: Surface) -> Lease {
        Lease { leases: self.clone(), surface }
    }

    /// `surface` asks for sensors in this frame: its interest is created on
    /// the first ask and kept by every frame that asks again.
    pub fn ask(&self, surface: Surface) -> Sensors {
        let mut held = self.held();
        let entry = held.entry(surface).or_insert_with(|| Held { interest: Arc::new(self.0.service.interest()), asked: false });
        entry.asked = true;
        Sensors(entry.interest.clone())
    }

    /// Ends a frame: drops the interest of every surface that did not ask in
    /// it and returns those surfaces. The others keep theirs for the next
    /// frame.
    pub fn end_frame(&self) -> Vec<Surface> {
        let gone = self.take(|_, h| !h.asked);
        for h in self.held().values_mut() {
            h.asked = false;
        }
        gone.into_iter().map(|(surface, _)| surface).collect()
    }

    /// Drops the interests of the add-on `id` (it was turned off or stopped).
    pub fn release(&self, id: &str) {
        drop(self.take(|s, _| s.addon() == Some(id)));
    }

    /// The sensors an add-on's surface holds now, if any, without asking for
    /// them: for an action that works between frames. Its page's when it
    /// holds one, else its tile's.
    pub fn held_by(&self, id: &'static str) -> Option<Sensors> {
        let held = self.held();
        [Surface::Page(id), Surface::Tile(id)].iter().find_map(|s| held.get(s)).map(|h| Sensors(h.interest.clone()))
    }

    /// "Refresh": reads every sensor at once through every interest held,
    /// whichever surface holds it (a page, a tile, the core Summary).
    pub fn read_now(&self) {
        let interests: Vec<Arc<Interest>> = self.held().values().map(|h| h.interest.clone()).collect();
        for interest in interests {
            interest.read_now();
        }
    }

    /// The map of held interests; a panic elsewhere does not lock it out.
    fn held(&self) -> MutexGuard<'_, HashMap<Surface, Held>> {
        self.0.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Removes the surfaces `pick` selects and returns them with their
    /// interests, to be dropped without the lock.
    fn take(&self, pick: impl Fn(Surface, &Held) -> bool) -> Vec<(Surface, Held)> {
        let mut held = self.held();
        let gone: Vec<Surface> = held.iter().filter(|(s, h)| pick(**s, h)).map(|(s, _)| *s).collect();
        gone.into_iter().filter_map(|s| held.remove(&s).map(|h| (s, h))).collect()
    }
}
