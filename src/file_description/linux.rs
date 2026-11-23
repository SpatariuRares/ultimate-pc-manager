//! File descriptions on Linux: no version resource, so none.

/// No version resources: no file description.
pub fn file_description(_: &str) -> Option<String> {
    None
}
