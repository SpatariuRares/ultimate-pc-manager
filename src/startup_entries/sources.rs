//! Several sources of startup entries at once.
//!
//! A system can start programs in more than one way (on Linux: XDG autostart
//! files and systemd user units). Each way is a [`StartupSource`]; their
//! entries are listed together, each marked with its source, and a source
//! that cannot be read does not hide the others.

use super::StartupEntry;

/// Said when a change names an entry no source knows.
const GONE: &str = "That startup app is no longer in the list";

/// One way the system starts programs.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub trait StartupSource {
    /// Names this source in [`StartupEntry::source`].
    fn id(&self) -> &'static str;
    /// The source's entries, or why it could not be read.
    fn list(&self) -> Result<Vec<StartupEntry>, String>;
    /// Turns one of this source's entries on or off.
    fn set_enabled(&self, entry: &StartupEntry, enable: bool) -> Result<(), String>;
}

/// The entries of every source, in the sources' order, and the sources that
/// could not be read with why (they contribute nothing).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn list_all(sources: &[Box<dyn StartupSource>]) -> (Vec<StartupEntry>, Vec<(&'static str, String)>) {
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for source in sources {
        match source.list() {
            Ok(mut found) => {
                for e in &mut found {
                    e.source = source.id().to_string();
                }
                entries.append(&mut found);
            }
            Err(why) => failures.push((source.id(), why)),
        }
    }
    (entries, failures)
}

/// Gives the change to the source that read `entry`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn set_enabled_in(sources: &[Box<dyn StartupSource>], entry: &StartupEntry, enable: bool) -> Result<(), String> {
    match sources.iter().find(|s| s.id() == entry.source) {
        Some(source) => source.set_enabled(entry, enable),
        None => Err(GONE.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        id: &'static str,
        result: Result<Vec<&'static str>, &'static str>,
    }

    fn boxed(id: &'static str, result: Result<Vec<&'static str>, &'static str>) -> Box<dyn StartupSource> {
        Box::new(Fake { id, result })
    }

    impl StartupSource for Fake {
        fn id(&self) -> &'static str {
            self.id
        }
        fn list(&self) -> Result<Vec<StartupEntry>, String> {
            match &self.result {
                Ok(names) => Ok(names.iter().map(|n| StartupEntry { name: n.to_string(), ..Default::default() }).collect()),
                Err(why) => Err(why.to_string()),
            }
        }
        fn set_enabled(&self, entry: &StartupEntry, _enable: bool) -> Result<(), String> {
            Err(format!("{} got {}", self.id, entry.name))
        }
    }

    #[test]
    fn every_entry_carries_its_source_and_the_lists_are_shown_together() {
        let sources = vec![boxed("a", Ok(vec!["one", "two"])), boxed("b", Ok(vec!["three"]))];
        let (entries, failures) = list_all(&sources);
        let seen: Vec<_> = entries.iter().map(|e| (e.source.as_str(), e.name.as_str())).collect();
        assert_eq!(seen, [("a", "one"), ("a", "two"), ("b", "three")]);
        assert!(failures.is_empty());
    }

    #[test]
    fn a_source_that_fails_does_not_hide_the_other() {
        let sources = vec![boxed("a", Err("no bus")), boxed("b", Ok(vec!["three"]))];
        let (entries, failures) = list_all(&sources);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "b");
        assert_eq!(failures, [("a", "no bus".to_string())]);
    }

    #[test]
    fn a_change_goes_to_the_source_that_read_the_entry() {
        let sources = vec![boxed("a", Ok(vec![])), boxed("b", Ok(vec![]))];
        let entry = StartupEntry { name: "x".into(), source: "b".into(), ..Default::default() };
        assert_eq!(set_enabled_in(&sources, &entry, false), Err("b got x".to_string()));
        let gone = StartupEntry { source: "c".into(), ..Default::default() };
        assert_eq!(set_enabled_in(&sources, &gone, true), Err(GONE.to_string()));
    }
}
