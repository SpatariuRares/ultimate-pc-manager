//! The machine facts System Info shows on Linux, read through an
//! [`FsRoot`] and an injected environment lookup, so tests run on any host
//! against fixture trees.
//!
//! A fact the system does not expose is `None` and its field is left out of
//! the page: nothing is shown empty. Only files readable without root are
//! used (no serial numbers, no `product_uuid`).

use crate::fs_root::FsRoot;

/// Everything System Info learns from a Linux system.
#[derive(Debug, Default, PartialEq)]
pub struct LinuxFacts {
    /// `PRETTY_NAME` of `/etc/os-release` (else `NAME`).
    pub distribution: Option<String>,
    /// `VERSION_ID` of `/etc/os-release`.
    pub version: Option<String>,
    /// `/proc/sys/kernel/osrelease`.
    pub kernel: Option<String>,
    /// "Wayland" or "X11".
    pub session: Option<String>,
    /// `XDG_CURRENT_DESKTOP`.
    pub desktop: Option<String>,
    pub host_name: Option<String>,
    /// Seconds, from `/proc/uptime`.
    pub uptime: Option<u64>,
    /// The machine's maker: DMI `sys_vendor`.
    pub vendor: Option<String>,
    /// The board: DMI vendor and name, else the device-tree model.
    pub board: Option<String>,
    /// The firmware: DMI BIOS vendor and version.
    pub bios: Option<String>,
}

impl LinuxFacts {
    /// Reads the facts under `root`; `env` looks up an environment variable.
    pub fn read(root: &FsRoot, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let os = root.read_first(&["etc/os-release", "usr/lib/os-release"]).unwrap_or_default();
        let os_value = |key: &str| {
            os.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(|v| v.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty())
        };
        let non_empty = |v: Option<String>| v.filter(|v| !v.is_empty());
        let session = match non_empty(env("XDG_SESSION_TYPE")).map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("wayland") => Some("Wayland".to_string()),
            Some("x11") => Some("X11".to_string()),
            Some(_) => None,
            None => non_empty(env("WAYLAND_DISPLAY")).map(|_| "Wayland".to_string()),
        };
        // The DMI tables are an x86/UEFI interface: a probe, not an
        // architecture check, decides whether to read them; boards that
        // describe themselves by a device tree answer there instead.
        let dmi = root.probe("sys/class/dmi/id");
        let dmi_value = |name: &str| if dmi { root.read(&format!("sys/class/dmi/id/{name}")) } else { None };
        let join = |a: Option<String>, b: Option<String>| match (a, b) {
            (Some(a), Some(b)) => Some(format!("{a} {b}")),
            (a, b) => a.or(b),
        };
        let board = join(dmi_value("board_vendor"), dmi_value("board_name")).or_else(|| root.read("sys/firmware/devicetree/base/model"));
        Self {
            distribution: os_value("PRETTY_NAME").or_else(|| os_value("NAME")),
            version: os_value("VERSION_ID"),
            kernel: root.read("proc/sys/kernel/osrelease"),
            session,
            desktop: non_empty(env("XDG_CURRENT_DESKTOP")),
            host_name: root.read_first(&["proc/sys/kernel/hostname", "etc/hostname"]),
            uptime: root.read("proc/uptime").and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok()).map(|s| s as u64),
            vendor: dmi_value("sys_vendor"),
            board,
            bios: join(dmi_value("bios_vendor"), dmi_value("bios_version")),
        }
    }

    /// The facts as System Info fields (named like the page's own), in page order.
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        let items: [(&'static str, Option<String>); 10] = [
            ("Computer name", self.host_name.clone()),
            ("Operating system", self.distribution.clone()),
            ("Version", self.version.clone()),
            ("Kernel", self.kernel.clone()),
            ("Up time", self.uptime.map(crate::widgets::fmt_uptime)),
            ("Session", self.session.clone()),
            ("Desktop", self.desktop.clone()),
            ("Vendor", self.vendor.clone()),
            ("Board", self.board.clone()),
            ("BIOS", self.bios.clone()),
        ];
        items.into_iter().filter_map(|(k, v)| Some((k, v?))).collect()
    }
}

/// The running system's facts (production: root `/`, the process environment).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn system_fields() -> Vec<(&'static str, String)> {
    LinuxFacts::read(&FsRoot::system(), &|k| std::env::var(k).ok()).fields()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn a_desktop_tree_gives_every_field() {
        let f = LinuxFacts::read(&fixture("desktop"), &env(&[("XDG_SESSION_TYPE", "wayland"), ("XDG_CURRENT_DESKTOP", "GNOME")]));
        assert_eq!(f.distribution.as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(f.version.as_deref(), Some("24.04"));
        assert_eq!(f.kernel.as_deref(), Some("6.8.0-45-generic"));
        assert_eq!((f.session.as_deref(), f.desktop.as_deref()), (Some("Wayland"), Some("GNOME")));
        assert_eq!(f.host_name.as_deref(), Some("workstation"));
        assert_eq!(f.uptime, Some(90_061));
        assert_eq!(f.vendor.as_deref(), Some("ASUS"));
        assert_eq!(f.board.as_deref(), Some("ASUSTeK COMPUTER INC. PRIME B450M-A"));
        assert_eq!(f.bios.as_deref(), Some("American Megatrends Inc. 3801"));
    }

    #[test]
    fn the_fields_carry_the_page_names_and_the_formatted_uptime() {
        let fields = LinuxFacts::read(&fixture("desktop"), &env(&[("XDG_SESSION_TYPE", "x11")])).fields();
        let get = |k: &str| fields.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("Up time"), Some("1:01:01:01"));
        assert_eq!(get("Session"), Some("X11"));
        assert_eq!(get("Desktop"), None, "no desktop variable: the field is hidden");
    }

    #[test]
    fn a_wayland_display_alone_names_a_wayland_session() {
        let f = LinuxFacts::read(&fixture("desktop"), &env(&[("WAYLAND_DISPLAY", "wayland-0")]));
        assert_eq!(f.session.as_deref(), Some("Wayland"));
    }

    #[test]
    fn without_the_dmi_interface_the_board_falls_back_to_the_device_tree() {
        let root = fixture("arm-board");
        assert!(!root.probe("sys/class/dmi/id"));
        let f = LinuxFacts::read(&root, &env(&[]));
        assert_eq!(f.board.as_deref(), Some("Raspberry Pi 4 Model B Rev 1.4"));
        assert_eq!((f.vendor, f.bios), (None, None));
        assert_eq!(f.host_name.as_deref(), Some("pi"), "etc/hostname when the proc file is missing");
        assert_eq!(f.distribution.as_deref(), Some("Debian GNU/Linux 12 (bookworm)"));
        assert_eq!((f.session, f.desktop), (None, None));
    }

    #[test]
    fn fields_the_system_does_not_expose_are_not_listed() {
        let names: Vec<_> = LinuxFacts::read(&fixture("arm-board"), &env(&[])).fields().into_iter().map(|(k, _)| k).collect();
        assert_eq!(names, ["Computer name", "Operating system", "Version", "Kernel", "Up time", "Board"]);
    }

    #[test]
    fn an_empty_tree_gives_no_fields() {
        assert!(LinuxFacts::read(&fixture("does-not-exist"), &env(&[])).fields().is_empty());
    }
}
