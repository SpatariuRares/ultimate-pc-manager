//! Flight Recorder's own settings, saved under `addons.flight-recorder`
//! and kept while it is off: the trace folder, the last Replay folder and
//! the Summary tile.

use std::path::{Path, PathBuf};

use crate::addons::{is_default, is_true};

/// The settings: a default value is left out, and the fields it does not
/// know are kept as they were.
#[derive(serde::Deserialize, serde::Serialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Prefs {
    /// Where Save opens; empty means Documents.
    #[serde(rename = "traceFolder", skip_serializing_if = "is_default")]
    pub trace_folder: String,
    /// The folder of the last trace opened in Replay; empty means the
    /// trace folder.
    #[serde(rename = "replayFolder", skip_serializing_if = "is_default")]
    pub replay_folder: String,
    /// The Summary tile while recording (on by default).
    #[serde(rename = "summaryTile", skip_serializing_if = "is_true")]
    pub summary_tile: bool,
    /// The fields of newer versions, written back untouched.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { trace_folder: String::new(), replay_folder: String::new(), summary_tile: true, rest: serde_json::Map::new() }
    }
}

impl Prefs {
    /// The folder Save and "Export as CSV" open in: the one chosen, else
    /// Documents.
    pub fn trace_folder(&self) -> PathBuf {
        if self.trace_folder.is_empty() {
            documents()
        } else {
            PathBuf::from(&self.trace_folder)
        }
    }

    /// The folder Replay… opens in: the last one used, else the trace folder.
    pub fn replay_folder(&self) -> PathBuf {
        if self.replay_folder.is_empty() {
            self.trace_folder()
        } else {
            PathBuf::from(&self.replay_folder)
        }
    }

    /// Remembers the folder of `trace`, just opened in Replay.
    pub fn remember_replay(&mut self, trace: &Path) {
        if let Some(folder) = trace.parent().filter(|f| !f.as_os_str().is_empty()) {
            self.replay_folder = folder.to_string_lossy().into_owned();
        }
    }
}

/// The user's Documents folder (else the temp folder).
pub fn documents() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join("Documents"))
        .filter(|d| d.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::{read_settings, write_settings};
    use serde_json::json;

    #[test]
    fn nobody_changed_them_so_nothing_is_saved() {
        assert_eq!(write_settings(&Prefs::default()), serde_json::Value::Null);
    }

    #[test]
    fn they_read_and_write_under_their_names_keeping_unknown_fields() {
        let saved = json!({ "traceFolder": "D:\\Traces", "replayFolder": "E:\\Old", "summaryTile": false, "future": [1] });
        let prefs: Prefs = read_settings(&saved);
        assert_eq!(prefs.trace_folder(), PathBuf::from("D:\\Traces"));
        assert_eq!(prefs.replay_folder(), PathBuf::from("E:\\Old"));
        assert!(!prefs.summary_tile);
        assert_eq!(write_settings(&prefs), saved);
    }

    #[test]
    fn the_trace_folder_is_documents_and_replay_opens_there_until_used() {
        let prefs = Prefs::default();
        assert_eq!(prefs.trace_folder(), documents());
        assert_eq!(prefs.replay_folder(), documents());
        assert!(prefs.summary_tile, "the Summary tile is on by default");
    }

    #[test]
    fn replay_remembers_the_folder_of_the_trace_it_opened() {
        let mut prefs = Prefs::default();
        prefs.remember_replay(Path::new("C:\\Runs\\slow.upmtrace"));
        assert_eq!(prefs.replay_folder(), PathBuf::from("C:\\Runs"));
    }

    #[test]
    fn unreadable_settings_give_the_defaults() {
        assert_eq!(read_settings::<Prefs>(&json!("nonsense")), Prefs::default());
    }
}
