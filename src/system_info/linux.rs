//! Linux adds the facts read from `/proc`, `/sys` and `/etc`.

pub fn fields() -> Vec<(&'static str, String)> {
    crate::linux_facts::system_fields()
}
