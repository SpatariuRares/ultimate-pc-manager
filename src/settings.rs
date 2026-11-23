//! Settings saved between runs (a small JSON file in the user's app-data folder).

use serde::{Deserialize, Serialize};

/// Update speeds and their samples per second, fastest first: Fast 4/sec,
/// High 2/sec, Normal 1/sec, Slow 0.5/sec, Paused.
pub const SPEEDS: [(&str, u64); 5] = [
    ("Fast \u{2014} 4/sec", 250),
    ("High \u{2014} 2/sec", 500),
    ("Normal \u{2014} 1/sec", 1000),
    ("Slow \u{2014} 0.5/sec", 2000),
    ("Paused", 0),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark: bool,
    pub speed: usize,
    pub always_on_top: bool,
    pub start_page: String,
    /// The page shown when the app was closed (the app reopens there).
    pub last_page: String,
    pub bits_per_second: bool,
    pub fahrenheit: bool,
    pub cpu_color: [u8; 3],
    pub mem_color: [u8; 3],
    pub disk_color: [u8; 3],
    pub net_color: [u8; 3],
    pub accent: [u8; 3],
    /// Performance page: the device shown on the right ("cpu", "disk:0", ...).
    pub perf_device: String,
    /// CPU graph: one cell per logical processor ("CPUs Expanded") or overall.
    pub perf_logical: bool,
    /// Draw kernel time under CPU utilisation ("Show kernel times").
    pub perf_kernel: bool,
    /// GPU graph: one cell per engine ("Engines") or overall.
    pub perf_gpu_engines: bool,
    /// Processes: children under their parent ("Process tree") or a flat
    /// list.
    pub process_tree: bool,
    /// Processes: a process that ends stays a short while, tinted ("Show
    /// exited processes").
    pub show_exited: bool,
    /// Processes: each process's CPU is a moving average of its readings
    /// ("Smooth CPU readings").
    pub smooth_cpu: bool,
    /// Processes: a row whose figures changed is lit a moment ("Highlight
    /// Process Updates", a Settings item).
    pub highlight_updates: bool,
    /// Processes: Memory, Disk and Peak memory as a percent of the
    /// machine's ("Resource values ▸ Percents") instead of values.
    pub process_percents: bool,
    /// Processes: the table's columns and sort.
    pub process_table: ProcessTable,
    /// "Show Task Manager in Reports": Processes lists the app itself (off
    /// by default).
    pub show_self_in_reports: bool,
    /// "Hide Microsoft Services and Processes": Processes leaves out the
    /// programs with a verified Microsoft Windows signature.
    pub hide_microsoft: bool,
    /// "Ask before applying Efficiency mode": Efficiency mode asks first
    /// (on by default; "Don't ask again" turns it off).
    pub ask_efficiency: bool,
    /// Add-ons by id: `{ "enabled": bool, "settings": <opaque>, ... }`.
    /// Missing means on. Read and written through `AddonHost`.
    pub addons: serde_json::Value,
    /// Root fields this version does not know (written by a newer one),
    /// kept so that saving does not erase them.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
    /// The saved `dark` while `--theme` replaces it for this run.
    #[serde(skip)]
    saved_dark: Option<bool>,
}

/// What a page changed in the settings: the settings it was drawn with and
/// the ones it asks for. Applying it changes only the fields that differ
/// between the two, so a change made in the same frame by another page (or
/// by an add-on) is never undone.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsChange {
    before: Settings,
    after: Settings,
}

impl SettingsChange {
    /// The change from `before` (what the page was drawn with) to `after`.
    pub fn new(before: &Settings, after: Settings) -> Self {
        SettingsChange { before: before.clone(), after }
    }

    /// Sets, in `current`, every field this change changed.
    pub fn apply(&self, current: &mut Settings) {
        let (Ok(serde_json::Value::Object(before)), Ok(serde_json::Value::Object(after)), Ok(serde_json::Value::Object(mut now))) =
            (serde_json::to_value(&self.before), serde_json::to_value(&self.after), serde_json::to_value(&*current))
        else {
            return;
        };
        for key in before.keys().chain(after.keys()) {
            if before.get(key) != after.get(key) {
                match after.get(key) {
                    Some(value) => now.insert(key.clone(), value.clone()),
                    None => now.remove(key),
                };
            }
        }
        let saved_dark = if self.before.saved_dark != self.after.saved_dark { self.after.saved_dark } else { current.saved_dark };
        if let Ok(mut merged) = serde_json::from_value::<Settings>(serde_json::Value::Object(now)) {
            merged.saved_dark = saved_dark;
            *current = merged;
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dark: true,
            speed: 2,
            always_on_top: false,
            start_page: "Last used".into(),
            last_page: "summary".into(),
            bits_per_second: true,
            fahrenheit: false,
            cpu_color: GraphColor::Cpu.original(),
            mem_color: GraphColor::Memory.original(),
            disk_color: GraphColor::Disk.original(),
            net_color: GraphColor::Network.original(),
            accent: GraphColor::Values.original(),
            perf_device: "cpu".into(),
            perf_logical: true,
            perf_kernel: true,
            perf_gpu_engines: true,
            process_tree: true,
            show_exited: true,
            smooth_cpu: false,
            highlight_updates: true,
            process_percents: false,
            process_table: ProcessTable::default(),
            show_self_in_reports: false,
            hide_microsoft: false,
            ask_efficiency: true,
            addons: serde_json::Value::Object(Default::default()),
            unknown: Default::default(),
            saved_dark: None,
        }
    }
}

/// The Processes table's saved layout: column order, hidden columns,
/// column widths and the sort column and direction. Columns are named by
/// their keys (the Processes page's column table); a key this version does
/// not know is kept, so a newer version's columns survive a save.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessTable {
    /// Every column the user has laid out, hidden ones too, left to right.
    /// Empty until the user moves, hides or shows a column; a column
    /// missing from it is new and takes its default place and visibility.
    pub column_order: Vec<String>,
    /// The columns of `column_order` the user hid.
    pub hidden_columns: Vec<String>,
    /// Widths in points of the columns the user resized.
    pub column_widths: std::collections::BTreeMap<String, f32>,
    /// The key of the column the table is sorted by.
    pub sort_column: String,
    /// "descending" (largest first) or "ascending".
    pub sort_direction: String,
    /// The Process Inspector window's width in points, as last resized.
    pub inspector_width: f32,
    /// The File activity window's width in points, as last resized.
    pub file_activity_width: f32,
}

/// The Process Inspector's width before the user resizes it.
pub const INSPECTOR_WIDTH: f32 = 620.0;

/// The File activity window's width before the user resizes it.
pub const FILE_ACTIVITY_WIDTH: f32 = 760.0;

impl Default for ProcessTable {
    /// All columns visible, sorted by Memory, largest first.
    fn default() -> Self {
        ProcessTable {
            column_order: Vec::new(),
            hidden_columns: Vec::new(),
            column_widths: Default::default(),
            sort_column: "memory".into(),
            sort_direction: "descending".into(),
            inspector_width: INSPECTOR_WIDTH,
            file_activity_width: FILE_ACTIVITY_WIDTH,
        }
    }
}

/// The name of the app's data folders, in %APPDATA% and %LOCALAPPDATA%.
pub const DATA_FOLDER: &str = crate::app_folders::APP_FOLDER;

/// Their name before the app was renamed "Ultimate PC Manager".
const OLD_DATA_FOLDER: &str = "MyTaskManager";

/// The app's data folder (%APPDATA%\UltimatePCManager), where its settings and
/// the add-ons' files live.
pub fn app_dir() -> Option<std::path::PathBuf> {
    crate::app_folders::config()
}

/// Renames the data folders of the old name ("MyTaskManager") to the new
/// one, once, so settings and histories survive the rename. A folder of the
/// new name, once there, is left as it is.
pub fn move_old_data_folders() {
    for base in crate::app_folders::legacy_bases() {
        let (old, new) = (base.join(OLD_DATA_FOLDER), base.join(DATA_FOLDER));
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(old, new);
        }
    }
}

/// A graph colour the settings keep, one row on the Colors page. Its
/// default is the built-in palette (`theme::GraphPalette`); the user can
/// still pick their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphColor {
    Cpu,
    Memory,
    Disk,
    Network,
    /// Figures and values next to graphs (saved as `accent`).
    Values,
}

impl GraphColor {
    pub const ALL: [GraphColor; 5] =
        [GraphColor::Cpu, GraphColor::Memory, GraphColor::Disk, GraphColor::Network, GraphColor::Values];

    pub fn name(self) -> &'static str {
        match self {
            GraphColor::Cpu => "CPU",
            GraphColor::Memory => "Memory",
            GraphColor::Disk => "Disk",
            GraphColor::Network => "Network",
            GraphColor::Values => "Figures and values",
        }
    }

    /// What the colour paints, under its name.
    pub fn sub(self) -> &'static str {
        match self {
            GraphColor::Cpu => "Graph frame",
            GraphColor::Memory => "",
            GraphColor::Disk => "Read / active time",
            GraphColor::Network => "",
            GraphColor::Values => "Accent",
        }
    }

    /// The row's built-in colour, from the default palette.
    pub fn original(self) -> [u8; 3] {
        let g = crate::theme::Theme::of(crate::theme::Mode::Dark).graphs;
        let c = match self {
            GraphColor::Cpu => g.cpu,
            GraphColor::Memory => g.memory,
            GraphColor::Disk => g.disk,
            GraphColor::Network => g.network,
            GraphColor::Values => g.value,
        };
        [c.r(), c.g(), c.b()]
    }

    /// The default of earlier versions (Windows' classic colours and the old
    /// accent). Saved unchanged, it means the user never chose a colour.
    fn old_default(self) -> [u8; 3] {
        match self {
            GraphColor::Cpu => [0x11, 0x7d, 0xbb],
            GraphColor::Memory => [0x8b, 0x12, 0xae],
            GraphColor::Disk => [0x4d, 0xa6, 0x0c],
            GraphColor::Network => [0xa7, 0x4f, 0x01],
            GraphColor::Values => [0x60, 0xa5, 0xfa],
        }
    }

    pub fn get(self, s: &Settings) -> [u8; 3] {
        match self {
            GraphColor::Cpu => s.cpu_color,
            GraphColor::Memory => s.mem_color,
            GraphColor::Disk => s.disk_color,
            GraphColor::Network => s.net_color,
            GraphColor::Values => s.accent,
        }
    }

    pub fn get_mut(self, s: &mut Settings) -> &mut [u8; 3] {
        match self {
            GraphColor::Cpu => &mut s.cpu_color,
            GraphColor::Memory => &mut s.mem_color,
            GraphColor::Disk => &mut s.disk_color,
            GraphColor::Network => &mut s.net_color,
            GraphColor::Values => &mut s.accent,
        }
    }

    pub fn is_original(self, s: &Settings) -> bool {
        self.get(s) == self.original()
    }

    /// "Use default": this row goes back to its built-in colour.
    pub fn use_original(self, s: &mut Settings) {
        *self.get_mut(s) = self.original();
    }
}

/// The settings file. Tests have none: nothing they do reads or writes the
/// user's real settings.
fn path() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(app_dir()?.join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| Self::from_json(&s))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, self.to_json());
        }
    }

    /// Reads the settings file's text. Text that is not a JSON object gives
    /// the defaults; a field that cannot be read falls back to its default
    /// alone, so one bad value never erases the others.
    /// Graph colours still at an earlier version's default move to the
    /// built-in palette.
    pub fn from_json(text: &str) -> Self {
        let mut s = Self::parse(text);
        s.migrate_colors();
        s
    }

    /// A saved colour equal to an earlier version's default was never chosen
    /// by the user: it becomes the built-in one. Any other colour is a choice
    /// and stays.
    fn migrate_colors(&mut self) {
        for c in GraphColor::ALL {
            if c.get(self) == c.old_default() {
                c.use_original(self);
            }
        }
    }

    fn parse(text: &str) -> Self {
        let Ok(serde_json::Value::Object(raw)) = serde_json::from_str::<serde_json::Value>(text) else {
            return Self::default();
        };
        if let Ok(s) = serde_json::from_value(serde_json::Value::Object(raw.clone())) {
            return s;
        }
        let mut good = match serde_json::to_value(Self::default()) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => return Self::default(),
        };
        for (key, value) in raw {
            let previous = good.insert(key.clone(), value);
            if serde_json::from_value::<Self>(serde_json::Value::Object(good.clone())).is_err() {
                match previous {
                    Some(v) => good.insert(key, v),
                    None => good.remove(&key),
                };
            }
        }
        serde_json::from_value(serde_json::Value::Object(good)).unwrap_or_default()
    }

    /// The settings file's text, unknown fields included.
    pub fn to_json(&self) -> String {
        match self.saved_dark {
            Some(dark) => serde_json::to_string_pretty(&Settings { dark, ..self.clone() }),
            None => serde_json::to_string_pretty(self),
        }
        .unwrap_or_default()
    }

    /// Uses the dark or light theme for this run only (`--theme`): saving
    /// keeps the theme that was saved.
    pub fn theme_for_this_run(&mut self, dark: bool) {
        self.saved_dark.get_or_insert(self.dark);
        self.dark = dark;
    }

    /// The user picks the theme: it is saved, even during a `--theme` run.
    pub fn set_dark(&mut self, dark: bool) {
        self.saved_dark = None;
        self.dark = dark;
    }

    /// The theme of the dark or light setting, its graph colours the saved
    /// ones (CPU, memory, disk, network and values). Pages draw graphs with
    /// it, so a colour picked on Colors applies everywhere; the colours the
    /// settings don't keep stay the built-in ones.
    pub fn theme(&self) -> crate::theme::Theme {
        let mut t = crate::theme::Theme::of(crate::theme::Mode::from_dark(self.dark));
        let colour = |c: GraphColor| {
            let [r, g, b] = c.get(self);
            egui::Color32::from_rgb(r, g, b)
        };
        let g = &mut t.graphs;
        (g.cpu, g.memory, g.disk, g.network, g.value) =
            (colour(GraphColor::Cpu), colour(GraphColor::Memory), colour(GraphColor::Disk), colour(GraphColor::Network), colour(GraphColor::Values));
        t
    }

    pub fn interval_ms(&self) -> u64 {
        SPEEDS.get(self.speed).map(|s| s.1).unwrap_or(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_root_fields_survive_a_read_save_cycle() {
        let text = r#"{ "dark": false, "from_a_newer_version": { "x": 1 }, "addons": {} }"#;
        let s = Settings::from_json(text);
        assert!(!s.dark);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["from_a_newer_version"], json!({ "x": 1 }));
        assert_eq!(saved["dark"], json!(false));
    }

    #[test]
    fn a_bad_global_field_does_not_erase_the_addons_or_unknown_fields() {
        let text = r#"{ "speed": "fast", "kept": 7,
            "addons": { "drivers": { "enabled": false, "settings": { "sort": "date" } } } }"#;
        let s = Settings::from_json(text);
        assert_eq!(s.speed, Settings::default().speed);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["addons"]["drivers"], json!({ "enabled": false, "settings": { "sort": "date" } }));
        assert_eq!(saved["kept"], json!(7));
    }

    #[test]
    fn the_process_tree_setting_is_saved_and_starts_on() {
        assert!(Settings::default().process_tree, "a fresh install shows the tree");
        let s = Settings::from_json(r#"{ "process_tree": false }"#);
        assert!(!s.process_tree);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["process_tree"], json!(false));
        assert!(Settings::from_json(r#"{ "process_tree": "yes" }"#).process_tree, "a bad value falls back to on");
    }

    #[test]
    fn the_two_process_hiding_settings_start_off_and_are_saved() {
        let d = Settings::default();
        assert!(!d.show_self_in_reports && !d.hide_microsoft);
        let s = Settings::from_json(r#"{ "show_self_in_reports": true, "hide_microsoft": true }"#);
        assert!(s.show_self_in_reports && s.hide_microsoft);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!((saved["show_self_in_reports"].clone(), saved["hide_microsoft"].clone()), (json!(true), json!(true)));
        assert!(!Settings::from_json(r#"{ "hide_microsoft": 3 }"#).hide_microsoft, "a bad value falls back to off");
    }

    #[test]
    fn ask_before_applying_efficiency_mode_starts_on_and_is_saved() {
        assert!(Settings::default().ask_efficiency, "a fresh install asks first");
        let s = Settings::from_json(r#"{ "ask_efficiency": false }"#);
        assert!(!s.ask_efficiency);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["ask_efficiency"], json!(false));
        assert!(Settings::from_json(r#"{ "process_tree": false }"#).ask_efficiency, "an older file gets it on");
    }

    #[test]
    fn the_show_exited_processes_setting_is_saved_and_starts_on() {
        assert!(Settings::default().show_exited, "a fresh install keeps exited processes a moment");
        let s = Settings::from_json(r#"{ "show_exited": false }"#);
        assert!(!s.show_exited);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["show_exited"], json!(false));
        assert!(Settings::from_json(r#"{ "process_tree": false }"#).show_exited, "an older file gets it on");
    }

    #[test]
    fn the_smooth_cpu_readings_setting_is_saved_and_starts_off() {
        assert!(!Settings::default().smooth_cpu, "a fresh install shows the last reading");
        let s = Settings::from_json(r#"{ "smooth_cpu": true }"#);
        assert!(s.smooth_cpu);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["smooth_cpu"], json!(true));
        assert!(!Settings::from_json(r#"{ "process_tree": false }"#).smooth_cpu, "an older file gets it off");
    }

    #[test]
    fn the_highlight_process_updates_setting_is_saved_and_starts_on() {
        assert!(Settings::default().highlight_updates, "a fresh install lights changed rows");
        let s = Settings::from_json(r#"{ "highlight_updates": false }"#);
        assert!(!s.highlight_updates);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["highlight_updates"], json!(false));
        assert!(Settings::from_json(r#"{ "process_tree": false }"#).highlight_updates, "an older file gets it on");
    }

    #[test]
    fn the_resource_values_choice_is_saved_and_starts_on_values() {
        assert!(!Settings::default().process_percents, "a fresh install shows values");
        let s = Settings::from_json(r#"{ "process_percents": true }"#);
        assert!(s.process_percents);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["process_percents"], json!(true));
        assert!(!Settings::from_json(r#"{ "process_tree": false }"#).process_percents, "an older file gets values");
    }

    #[test]
    fn the_inspector_width_survives_a_save_and_older_files_get_the_default() {
        let mut s = Settings::default();
        assert_eq!(s.process_table.inspector_width, INSPECTOR_WIDTH);
        s.process_table.inspector_width = 744.0;
        assert_eq!(Settings::from_json(&s.to_json()).process_table.inspector_width, 744.0);
        let older = Settings::from_json(r#"{ "process_table": { "sort_column": "cpu" } }"#);
        assert_eq!(older.process_table.inspector_width, INSPECTOR_WIDTH);
        assert_eq!(older.process_table.sort_column, "cpu", "the other saved fields are kept");
    }

    #[test]
    fn the_file_activity_width_survives_a_save_and_older_files_get_the_default() {
        let mut s = Settings::default();
        assert_eq!(s.process_table.file_activity_width, FILE_ACTIVITY_WIDTH);
        s.process_table.file_activity_width = 812.0;
        assert_eq!(Settings::from_json(&s.to_json()).process_table.file_activity_width, 812.0);
        let older = Settings::from_json(r#"{ "process_table": { "inspector_width": 700.0 } }"#);
        assert_eq!(older.process_table.file_activity_width, FILE_ACTIVITY_WIDTH);
        assert_eq!(older.process_table.inspector_width, 700.0, "the other saved fields are kept");
    }

    fn hex(c: [u8; 3]) -> String {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    }

    fn saved(s: &Settings) -> [String; 5] {
        [hex(s.cpu_color), hex(s.mem_color), hex(s.disk_color), hex(s.net_color), hex(s.accent)]
    }

    const ORIGINAL: [&str; 5] = ["#31d1ff", "#be69ff", "#36d366", "#39b1ff", "#53befa"];

    #[test]
    fn the_default_graph_colours_are_the_built_in_palette() {
        assert_eq!(saved(&Settings::default()), ORIGINAL);
        let g = crate::theme::Theme::of(crate::theme::Mode::Dark).graphs;
        let d = Settings::default();
        let pairs = [(d.cpu_color, g.cpu), (d.mem_color, g.memory), (d.disk_color, g.disk), (d.net_color, g.network), (d.accent, g.value)];
        for (saved, theme) in pairs {
            assert_eq!(egui::Color32::from_rgb(saved[0], saved[1], saved[2]), theme, "settings and theme agree");
        }
    }

    #[test]
    fn the_old_default_colours_move_to_the_built_in_palette() {
        let text = r#"{ "dark": false, "cpu_color": [17, 125, 187], "mem_color": [139, 18, 174],
            "disk_color": [77, 166, 12], "net_color": [167, 79, 1], "accent": [96, 165, 250] }"#;
        let s = Settings::from_json(text);
        assert_eq!(saved(&s), ORIGINAL);
        assert!(!s.dark, "the other settings stay");
    }

    #[test]
    fn a_colour_the_user_chose_is_kept() {
        let text = r#"{ "cpu_color": [200, 10, 10], "mem_color": [139, 18, 174], "accent": [96, 165, 251] }"#;
        let s = Settings::from_json(text);
        assert_eq!(hex(s.cpu_color), "#c80a0a");
        assert_eq!(hex(s.mem_color), "#be69ff", "an old default next to it still moves");
        assert_eq!(hex(s.accent), "#60a5fb", "one step off the old default is a choice");
    }

    #[test]
    fn an_unreadable_colour_gives_the_original_without_losing_the_others() {
        let text = r#"{ "cpu_color": "blue", "disk_color": [1, 2, 3], "net_color": [167, 79, 1], "speed": 0 }"#;
        let s = Settings::from_json(text);
        assert_eq!(saved(&s), ["#31d1ff", "#be69ff", "#010203", "#39b1ff", "#53befa"]);
        assert_eq!(s.speed, 0);
    }

    #[test]
    fn use_original_puts_back_one_colour() {
        let mut s = Settings::from_json(r#"{ "cpu_color": [1, 1, 1], "disk_color": [2, 2, 2] }"#);
        GraphColor::Cpu.use_original(&mut s);
        assert_eq!(hex(s.cpu_color), "#31d1ff");
        assert_eq!(hex(s.disk_color), "#020202", "the other rows stay");
        assert!(GraphColor::Cpu.is_original(&s));
        assert!(!GraphColor::Disk.is_original(&s));
    }

    #[test]
    fn every_saved_colour_has_one_row() {
        let names: Vec<&str> = GraphColor::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names, ["CPU", "Memory", "Disk", "Network", "Figures and values"]);
        let originals: Vec<String> = GraphColor::ALL.iter().map(|c| hex(c.original())).collect();
        assert_eq!(originals, ORIGINAL);
    }

    #[test]
    fn text_that_is_not_json_gives_the_defaults() {
        let s = Settings::from_json("{ not json");
        assert_eq!(s.speed, Settings::default().speed);
        assert_eq!(s.addons, json!({}));
    }

    #[test]
    fn a_theme_for_this_run_is_used_but_not_saved() {
        let mut s = Settings::from_json(r#"{ "dark": true }"#);
        s.theme_for_this_run(false);
        assert!(!s.dark, "the run uses the asked theme");
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["dark"], json!(true), "the saved theme is kept");
        s.speed = 0;
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!((saved["dark"].clone(), saved["speed"].clone()), (json!(true), json!(0)), "other changes are saved");
    }

    #[test]
    fn the_theme_pages_draw_with_carries_the_saved_graph_colours() {
        let mut s = Settings { dark: false, ..Settings::default() };
        (s.cpu_color, s.mem_color, s.disk_color, s.net_color, s.accent) = ([1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12], [13, 14, 15]);
        let t = s.theme();
        let rgb = |c: egui::Color32| [c.r(), c.g(), c.b()];
        assert_eq!(t.mode, crate::theme::Mode::Light);
        assert_eq!(
            [rgb(t.graphs.cpu), rgb(t.graphs.memory), rgb(t.graphs.disk), rgb(t.graphs.network), rgb(t.graphs.value)],
            [[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12], [13, 14, 15]]
        );
        let original = crate::theme::GRAPHS;
        assert_eq!((t.graphs.gpu, t.graphs.kernel, t.graphs.power), (original.gpu, original.kernel, original.power), "colours the settings don't keep stay");
    }

    #[test]
    fn a_theme_the_user_picks_during_a_theme_run_is_saved() {
        let mut s = Settings::from_json(r#"{ "dark": true }"#);
        s.theme_for_this_run(false);
        s.set_dark(true);
        s.set_dark(false);
        assert!(!s.dark);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["dark"], json!(false), "the user's choice replaces the saved theme");
    }
}
