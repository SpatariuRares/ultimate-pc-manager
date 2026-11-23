//! The alerts as the add-on handles them: its commands (add, delete,
//! which rows are alerted), the watcher it keeps in step with them, and
//! their list in Settings › Power & Freq. The rules and their evaluator
//! are in [`super::alerts`].

use super::*;

impl PowerFreq {
    /// Alerts are offered: their line in [`crate::addons::FEATURE_TIERS`]
    /// ([`alerts::TIER_ID`]) is Free.
    pub fn alerts_offered(&self) -> bool {
        (self.tiers.0)(alerts::TIER_ID) == Tier::Free
    }

    /// The alerts set in Settings.
    pub fn alerts(&self) -> &[alerts::Rule] {
        &self.alerts
    }

    /// "Add alert": a new alert on the package temperature (the user then
    /// picks the sensor, the side, the value and the duration).
    pub fn add_alert(&mut self) {
        if !self.alerts_offered() {
            return;
        }
        self.alerts.push(alerts::Rule::new(PACKAGE_TEMPERATURE));
        self.sync_alerts();
    }

    /// The sensor `id` has a triggered alert (its row is highlighted).
    pub fn alerted(&self, id: &str) -> bool {
        self.alerted_ids().contains(id)
    }

    /// The metric ids with a triggered alert.
    pub(super) fn alerted_ids(&self) -> BTreeSet<String> {
        let tracked = self.watcher.as_ref().map(|w| w.tracked()).unwrap_or_default();
        tracked.into_iter().filter(|t| t.status.active).map(|t| t.rule.metric).collect()
    }

    /// Starts, updates or ends the alerts' watcher: it runs while the
    /// add-on is on, alerts are offered and at least one alert exists.
    pub(super) fn sync_alerts(&mut self) {
        let context = match &self.context {
            Some(c) if self.on && self.alerts_offered() && !self.alerts.is_empty() => c,
            _ => {
                self.watcher = None;
                return;
            }
        };
        match &self.watcher {
            Some(w) => {
                if w.rules() != self.alerts {
                    w.set_rules(self.alerts.clone());
                }
                w.set_fahrenheit(self.fahrenheit);
            }
            None => self.watcher = Some(watch::Watcher::start(context, self.alerts.clone(), self.fahrenheit)),
        }
    }

    /// "Delete" on the alert at `index`.
    pub fn delete_alert(&mut self, index: usize) {
        if index < self.alerts.len() {
            self.alerts.remove(index);
        }
        self.sync_alerts();
    }

    /// Settings › Power & Freq › Alerts: the list (sensor, above / below,
    /// value in the user's unit, duration, Delete, what it is doing) and
    /// "Add alert".
    pub(super) fn alerts_section(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let (sub, fahrenheit) = (page.sub_text(), page.settings.fahrenheit);
        let t = theme::of_ui(ui);
        ui.add_space(8.0);
        ui.label(RichText::new("Alerts").strong());
        ui.label(
            RichText::new("A Windows notification when a sensor stays past a value for the time set, and another once it is back for the same time. While an alert exists, the sensors are read in the background.")
                .size(11.0)
                .color(sub),
        );
        if !self.alerts_offered() {
            ui.label(RichText::new(NOT_INCLUDED).size(11.0).color(sub));
            return;
        }
        if let Some(why) = self.watcher.as_ref().and_then(|w| w.problem()) {
            ui.label(RichText::new(why).size(11.0).color(t.red));
        }
        let frame = self.watcher.as_ref().and_then(|w| w.frame());
        let tracked = self.watcher.as_ref().map(|w| w.tracked()).unwrap_or_default();
        let mut delete = None;
        for (i, rule) in self.alerts.iter_mut().enumerate() {
            let reading = frame.as_deref().and_then(|f| f.reading(&rule.metric));
            let unit = reading.map(|r| r.unit).or((rule.metric == PACKAGE_TEMPERATURE).then_some(Unit::Celsius));
            ui.horizontal(|ui| {
                let name = reading.map(alerts::sensor_name).unwrap_or_else(|| rule.metric.clone());
                egui::ComboBox::from_id_salt(("power_alert_sensor", i)).width(240.0).selected_text(name).show_ui(ui, |ui| {
                    let Some(f) = frame.as_deref() else {
                        ui.label(RichText::new(WAITING).color(sub));
                        return;
                    };
                    for r in f.readings().filter(|r| alerts::alertable(r.unit)) {
                        if ui.selectable_label(r.id == rule.metric, alerts::sensor_name(r)).clicked() && r.id != rule.metric {
                            if Some(r.unit) != unit {
                                // Another kind of value: start from its reading.
                                rule.threshold = r.value.unwrap_or(0.0);
                            }
                            rule.metric = r.id.clone();
                        }
                    }
                });
                egui::ComboBox::from_id_salt(("power_alert_side", i)).width(70.0).selected_text(rule.comparison.word()).show_ui(ui, |ui| {
                    ui.selectable_value(&mut rule.comparison, alerts::Comparison::Above, "above");
                    ui.selectable_value(&mut rule.comparison, alerts::Comparison::Below, "below");
                });
                match unit {
                    Some(unit) => {
                        let mut value = alerts::shown(unit, rule.threshold, fahrenheit);
                        if ui.add(egui::DragValue::new(&mut value).speed(0.1).max_decimals(2)).changed() {
                            rule.threshold = alerts::stored(unit, value, fahrenheit);
                        }
                        ui.label(alerts::entry_unit(unit, fahrenheit));
                    }
                    None => {
                        ui.label(RichText::new(WAITING).color(sub));
                    }
                }
                ui.label("for");
                ui.add(egui::DragValue::new(&mut rule.duration_s).range(0..=alerts::MAX_DURATION_S).suffix(" s"));
                if outline_button(ui, "Delete", true).clicked() {
                    delete = Some(i);
                }
            });
            match tracked.iter().find(|tr| tr.rule == *rule).map(|tr| &tr.status) {
                Some(alerts::Status { quiet: Some(why), .. }) => {
                    ui.label(RichText::new(format!("Quiet: {why}")).size(11.0).color(t.amber));
                }
                Some(alerts::Status { active: true, .. }) => {
                    ui.label(RichText::new("Triggered").size(11.0).color(t.red));
                }
                _ => {}
            }
        }
        self.fahrenheit = fahrenheit;
        if let Some(i) = delete {
            self.delete_alert(i);
        }
        if outline_button(ui, "Add alert", true).clicked() {
            self.add_alert();
        }
        self.sync_alerts();
    }
}
