//! Sensors pinned to the Summary tile (#193): the side panel's "Pin to
//! Summary" / "Unpin from Summary", the pinned order (saved under
//! `pinned`) and the lines shown under the tile.

use super::*;

/// The id of the pinned sensors' line in [`crate::addons::TIERS`].
pub const PINS_TIER_ID: &str = "power-freq.pins";

/// The side panel's link to pin the selected sensor.
pub const PIN: &str = "Pin to Summary";
/// The side panel's link to unpin it.
pub const UNPIN: &str = "Unpin from Summary";
/// Said of a pinned sensor the current frame does not have.
pub const NOT_ON_THIS_PC: &str = "not on this PC";

/// The Summary tile's lines for the pinned sensors, shown after
/// [`tile_lines`] in the pinned order: "Name: value" (an unread value
/// gives its reason), or "id: not on this PC" when the frame lacks it.
pub fn pinned_lines(frame: &Frame, pinned: &[String], fahrenheit: bool) -> Vec<String> {
    pinned
        .iter()
        .map(|id| match frame.reading(id) {
            Some(r) => format!("{}: {}", r.name, r.text_in(fahrenheit)),
            None => format!("{id}: {NOT_ON_THIS_PC}"),
        })
        .collect()
}

impl PowerFreq {
    /// Pins are offered: their line in [`crate::addons::TIERS`]
    /// ([`PINS_TIER_ID`]) is Free.
    pub fn pins_offered(&self) -> bool {
        (self.tiers.0)(PINS_TIER_ID) == Tier::Free
    }

    /// The pinned sensors (metric ids), in the order they were pinned.
    pub fn pinned(&self) -> &[String] {
        &self.pinned
    }

    /// The sensor `id` is pinned to the Summary tile.
    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned.iter().any(|p| p == id)
    }

    /// The side panel's link for `id`: [`PIN`] or [`UNPIN`].
    pub fn pin_label(&self, id: &str) -> &'static str {
        if self.is_pinned(id) {
            UNPIN
        } else {
            PIN
        }
    }

    /// Pins `id` (at the end) or, when pinned, unpins it; nothing while
    /// pins are not offered ([`Self::pins_offered`]).
    pub fn toggle_pin(&mut self, id: &str) {
        if !self.pins_offered() {
            return;
        }
        if self.is_pinned(id) {
            self.pinned.retain(|p| p != id);
        } else {
            self.pinned.push(id.to_string());
        }
    }

    /// Under the Summary tile's lines: the pinned sensors, each with its
    /// "Unpin" link (nothing while pins are not offered).
    pub(super) fn pinned_rows(&mut self, ui: &mut Ui, frame: &Frame, page: &mut PageContext) {
        if !self.pins_offered() {
            return;
        }
        let sub = page.sub_text();
        let mut unpin = None;
        for (id, line) in self.pinned.iter().zip(pinned_lines(frame, &self.pinned, page.settings.fahrenheit)) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(line).color(sub));
                if ui.small_button("Unpin").clicked() {
                    unpin = Some(id.clone());
                }
            });
        }
        if let Some(id) = unpin {
            self.toggle_pin(&id);
        }
    }
}
