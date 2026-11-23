//! The driver store: the packages Windows keeps, read with
//! `pnputil /enum-drivers /devices /format xml` and parsed from the XML,
//! never from localised text. Reading needs no administrator rights.
//!
//! This module is pure (text in, rows out); the Windows reader runs the
//! tool and measures the packages' folders.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use super::engine::parse_version;

/// A device that uses a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreDevice {
    pub instance_id: String,
    pub name: String,
}

/// One driver package of the store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StorePackage {
    /// The name Windows gave it in the store: "oem42.inf".
    pub published: String,
    /// The INF's own name: "nv_dispi.inf".
    pub original: String,
    pub provider: String,
    pub class: String,
    pub version: String,
    /// yyyy-mm-dd; empty when the tool's date could not be read.
    pub date: String,
    pub signer: String,
    /// The devices using the package.
    pub devices: Vec<StoreDevice>,
    /// Bytes the package's folder holds; `None` when it could not be found.
    pub size: Option<u64>,
}

impl StorePackage {
    /// Shipped by Microsoft, so never offered for removal. Judged by the
    /// provider, like the "Third-party" rule of the other tabs.
    pub fn inbox(&self) -> bool {
        self.provider.trim().to_lowercase().starts_with("microsoft")
    }

    /// A device uses it.
    pub fn in_use(&self) -> bool {
        !self.devices.is_empty()
    }

    /// Does the lowercase text filter `q` (empty: all) match the package's
    /// name, INF, provider, class or one of its devices?
    pub fn matches(&self, q: &str) -> bool {
        q.is_empty()
            || [&self.published, &self.original, &self.provider, &self.class].iter().any(|s| s.to_lowercase().contains(q))
            || self.devices.iter().any(|d| d.name.to_lowercase().contains(q) || d.instance_id.to_lowercase().contains(q))
    }
}

// --------------------------------------------------------------- reading

/// Reads the tool's XML into packages, in the tool's order.
pub fn parse_store(xml: &str) -> Result<Vec<StorePackage>, String> {
    let root = parse_xml(xml).map_err(|e| format!("pnputil did not return the driver store as XML ({e})"))?;
    if root.name != "PnpUtil" {
        return Err(format!("pnputil did not return the driver store as XML (found <{}>)", root.name));
    }
    let mut out = Vec::new();
    for d in root.children.iter().filter(|c| c.name == "Driver") {
        let published = d.attr("DriverName").ok_or("a driver package has no name in pnputil's output")?;
        let text = |name: &str| d.child(name).map(|c| c.text.trim().to_string()).unwrap_or_default();
        let (date, version) = split_version(&text("DriverVersion"));
        let devices = d
            .child("Devices")
            .map(|devs| {
                devs.children
                    .iter()
                    .filter(|c| c.name == "Device")
                    .map(|dev| StoreDevice {
                        instance_id: dev.attr("InstanceId").unwrap_or_default().to_string(),
                        name: dev.child("DeviceDescription").map(|c| c.text.trim().to_string()).unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(StorePackage {
            published: published.to_string(),
            original: text("OriginalName"),
            provider: text("ProviderName"),
            class: text("ClassName"),
            version,
            date,
            signer: text("SignerName"),
            devices,
            size: None,
        });
    }
    Ok(out)
}

/// "10/28/2024 24.30.0.2" -> ("2024-10-28", "24.30.0.2"). The tool writes
/// month/day/year; a first number above 12 can only be a day. A date that
/// cannot be read leaves the date empty and the text as the version.
fn split_version(s: &str) -> (String, String) {
    let Some((date, version)) = s.split_once(' ') else { return (String::new(), s.to_string()) };
    let parts: Vec<u32> = date.split('/').filter_map(|p| p.parse().ok()).collect();
    match parts[..] {
        [a, b, y] if date.matches('/').count() == 2 && y >= 1000 => {
            let (m, d) = if a > 12 { (b, a) } else { (a, b) };
            if (1..=12).contains(&m) && (1..=31).contains(&d) {
                return (format!("{y:04}-{m:02}-{d:02}"), version.trim().to_string());
            }
            (String::new(), s.to_string())
        }
        _ => (String::new(), s.to_string()),
    }
}

// ------------------------------------------------------- classification

/// What the page says about a package besides its columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// A newer package with the same original INF and provider exists.
    pub old: bool,
    /// At least one device uses it.
    pub in_use: bool,
    /// Shipped by Microsoft.
    pub inbox: bool,
}

impl Flags {
    /// The packages "Remove old packages" would remove: old, not in use,
    /// not inbox.
    pub fn removable_old(self) -> bool {
        self.old && !self.in_use && !self.inbox
    }

    /// The words shown for the flags, in a fixed order.
    pub fn words(self) -> Vec<&'static str> {
        let mut w = Vec::new();
        if self.old {
            w.push("Old");
        }
        if self.in_use {
            w.push("In use");
        }
        if self.inbox {
            w.push("Inbox");
        }
        w
    }
}

/// Which package is newer: the higher version, then the later date.
fn newer(a: &StorePackage, b: &StorePackage) -> Ordering {
    let by_version = match (parse_version(&a.version), parse_version(&b.version)) {
        (Some(mut x), Some(mut y)) => {
            let n = x.len().max(y.len());
            x.resize(n, 0);
            y.resize(n, 0);
            x.cmp(&y)
        }
        _ => Ordering::Equal,
    };
    by_version.then_with(|| a.date.cmp(&b.date))
}

/// The flags of each package, in the same order.
pub fn classify(packages: &[StorePackage]) -> Vec<Flags> {
    // The newest package of each (original INF, provider), compared without case.
    let mut newest: HashMap<(String, String), usize> = HashMap::new();
    for (i, p) in packages.iter().enumerate() {
        let key = (p.original.to_lowercase(), p.provider.to_lowercase());
        match newest.get(&key) {
            Some(&best) if newer(&packages[best], p) != Ordering::Less => {}
            _ => {
                newest.insert(key, i);
            }
        }
    }
    packages
        .iter()
        .map(|p| {
            let best = &packages[newest[&(p.original.to_lowercase(), p.provider.to_lowercase())]];
            Flags { old: newer(best, p) == Ordering::Greater, in_use: p.in_use(), inbox: p.inbox() }
        })
        .collect()
}

/// The four figures above the list, over the whole store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreFigures {
    pub packages: usize,
    /// Not inbox.
    pub third_party: usize,
    /// Packages "Remove old packages" would remove (see [`Flags::removable_old`]).
    pub old: usize,
    /// Bytes the packages hold; `None` when no package's folder was found.
    pub bytes: Option<u64>,
    /// Bytes the old packages hold; `None` when none of them has a size.
    pub freeable: Option<u64>,
}

impl StoreFigures {
    pub fn of(packages: &[StorePackage], flags: &[Flags]) -> Self {
        let sum = |it: &mut dyn Iterator<Item = u64>| -> Option<u64> {
            let sizes: Vec<u64> = it.collect();
            (!sizes.is_empty()).then(|| sizes.iter().sum())
        };
        StoreFigures {
            packages: packages.len(),
            third_party: flags.iter().filter(|f| !f.inbox).count(),
            old: flags.iter().filter(|f| f.removable_old()).count(),
            bytes: sum(&mut packages.iter().filter_map(|p| p.size)),
            freeable: sum(&mut packages.iter().zip(flags).filter(|(_, f)| f.removable_old()).filter_map(|(p, _)| p.size)),
        }
    }
}

/// What the Driver store tab shows for one read and one text filter:
/// worked out once and reused until either changes.
pub struct StoreList {
    /// The read it was built from (the page compares by pointer).
    pub packages: Arc<[StorePackage]>,
    /// The lowercase, trimmed filter it was built for.
    pub query: String,
    /// One per package, in the package order.
    pub flags: Vec<Flags>,
    /// Indexes of the packages shown: by original INF, newest first.
    pub rows: Vec<usize>,
    pub figures: StoreFigures,
}

impl StoreList {
    pub fn build(packages: &Arc<[StorePackage]>, query: &str) -> StoreList {
        let query = query.trim().to_lowercase();
        let flags = classify(packages);
        let mut rows: Vec<usize> = (0..packages.len()).filter(|&i| packages[i].matches(&query)).collect();
        rows.sort_by(|&a, &b| {
            let (x, y) = (&packages[a], &packages[b]);
            x.original.to_lowercase().cmp(&y.original.to_lowercase()).then_with(|| x.provider.to_lowercase().cmp(&y.provider.to_lowercase())).then_with(|| newer(y, x))
        });
        let figures = StoreFigures::of(packages, &flags);
        StoreList { packages: packages.clone(), query, flags, rows, figures }
    }

    /// Still right for this read and this filter text.
    pub fn is_for(&self, packages: &Arc<[StorePackage]>, query: &str) -> bool {
        Arc::ptr_eq(&self.packages, packages) && self.query == query.trim().to_lowercase()
    }
}

// ------------------------------------------------- selection and removal

/// The packages ticked on the Driver store tab, by published name. One model
/// for every action that works on "the selected packages". Inbox packages
/// cannot be ticked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection(std::collections::BTreeSet<String>);

impl Selection {
    /// Ticks or unticks `p`; an inbox package is never ticked.
    pub fn toggle(&mut self, p: &StorePackage) {
        if self.0.remove(&p.published) || p.inbox() {
            return;
        }
        self.0.insert(p.published.clone());
    }

    pub fn contains(&self, published: &str) -> bool {
        self.0.contains(published)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The ticked names, in name order.
    pub fn names(&self) -> Vec<String> {
        self.0.iter().cloned().collect()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Forgets what a new read of the store no longer holds.
    pub fn keep_only(&mut self, packages: &[StorePackage]) {
        self.0.retain(|name| packages.iter().any(|p| &p.published == name));
    }
}

/// The names "Remove old packages" removes: old, not in use, not inbox, in
/// the packages' order.
pub fn old_removable(packages: &[StorePackage]) -> Vec<String> {
    let flags = classify(packages);
    packages.iter().zip(flags).filter(|(_, f)| f.removable_old()).map(|(p, _)| p.published.clone()).collect()
}

/// The names "Back up drivers…" exports: every third-party package.
pub fn third_party(packages: &[StorePackage]) -> Vec<String> {
    packages.iter().filter(|p| !p.inbox()).map(|p| p.published.clone()).collect()
}

/// Why a plain removal is refused, in the words of the outcome.
pub const IN_USE_REASON: &str = "a device uses it";
/// Said where a package in use is shown: what a plain removal answers.
pub const NOT_REMOVED_IN_USE: &str = "Not removed: a device uses it.";

/// What a removal will run and what it refuses before running anything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemovalPlan {
    /// The packages to hand to `pnputil /delete-driver`, in the asked order.
    pub run: Vec<String>,
    /// The packages never sent to the tool, with the reason.
    pub refused: Vec<(String, String)>,
}

/// Which of `names` may be removed. Inbox packages are never removed, a
/// package in use is refused unless `force` is asked; `force` counts for a
/// single package only.
pub fn plan_removal(packages: &[StorePackage], names: &[String], force: bool) -> RemovalPlan {
    let force = force && names.len() == 1;
    let mut plan = RemovalPlan::default();
    let mut seen = std::collections::HashSet::new();
    for name in names.iter().filter(|n| seen.insert(n.as_str())) {
        match packages.iter().find(|p| &p.published == name) {
            None => plan.refused.push((name.clone(), "it is not in the driver store".into())),
            Some(p) if p.inbox() => plan.refused.push((name.clone(), "it ships with Windows".into())),
            Some(p) if p.in_use() && !force => plan.refused.push((name.clone(), IN_USE_REASON.into())),
            Some(_) => plan.run.push(name.clone()),
        }
    }
    plan
}

/// What the tool did with one package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageRun {
    pub name: String,
    /// The tool's exit code: 0 done, 3010 done but restart needed.
    pub code: i64,
    pub output: String,
}

impl PackageRun {
    fn done(&self) -> bool {
        matches!(self.code, 0 | 3010)
    }

    /// The tool's own words for a failure: its "Failed …" line, else the last
    /// line it wrote.
    fn reason(&self) -> String {
        let lines: Vec<&str> = self.output.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let line = lines.iter().find(|l| l.to_lowercase().starts_with("failed")).or(lines.last());
        match line {
            Some(l) => l.split_once(": ").filter(|(head, _)| head.to_lowercase().starts_with("failed")).map_or(*l, |(_, rest)| rest).trim_end_matches('.').to_string(),
            None => format!("pnputil returned code {}", self.code),
        }
    }
}

/// Reads a store job's result: one entry per package, in the order run.
pub fn parse_runs(v: &serde_json::Value) -> Result<Vec<PackageRun>, String> {
    if v["ok"].as_bool() != Some(true) {
        return Err(v["error"].as_str().filter(|e| !e.is_empty()).unwrap_or("unknown error").to_string());
    }
    Ok(v["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|i| PackageRun { name: i["name"].as_str().unwrap_or_default().to_string(), code: i["code"].as_i64().unwrap_or(-1), output: i["output"].as_str().unwrap_or_default().to_string() })
                .collect()
        })
        .unwrap_or_default())
}

/// What a removal reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalOutcome {
    /// The page's message; the flag marks an error.
    pub message: (String, bool),
    pub removed: usize,
    pub reboot: bool,
}

/// "Removed N packages, freed X." and one "Not removed: <name> (<reason>)"
/// per package left. `size_of` gives a package's measured size.
pub fn removal_outcome(refused: &[(String, String)], runs: &[PackageRun], size_of: impl Fn(&str) -> Option<u64>) -> RemovalOutcome {
    let removed: Vec<&PackageRun> = runs.iter().filter(|r| r.done()).collect();
    let mut lines = Vec::new();
    if !removed.is_empty() {
        let sizes: Vec<u64> = removed.iter().filter_map(|r| size_of(&r.name)).collect();
        let mut line = format!("Removed {}", crate::addons::plural(removed.len(), "package"));
        if !sizes.is_empty() {
            line.push_str(&format!(", freed {}", crate::widgets::fmt_bytes(sizes.iter().sum())));
        }
        lines.push(line + ".");
    }
    let failures = refused.iter().cloned().chain(runs.iter().filter(|r| !r.done()).map(|r| (r.name.clone(), r.reason())));
    let mut any_left = false;
    for (name, why) in failures {
        any_left = true;
        lines.push(format!("Not removed: {name} ({why})"));
    }
    RemovalOutcome { message: (lines.join("\n"), removed.is_empty() && any_left), removed: removed.len(), reboot: removed.iter().any(|r| r.code == 3010) }
}

/// The dated subfolder of the folder the user picked: "<folder>\Drivers
/// 2026-11-23".
pub fn backup_folder(picked: &str, (year, month, day): (i64, u32, u32)) -> String {
    format!("{}\\Drivers {year:04}-{month:02}-{day:02}", picked.trim_end_matches(['\\', '/']))
}

/// The note on how a backup is used again.
pub const BACKUP_RESTORE_NOTE: &str = "Restore a backup with \u{201c}Install from an INF file\u{2026}\u{201d}, one INF per package.";

/// "Backed up N driver packages to <folder>." and one "Not backed up:
/// <name> (<reason>)" per failure.
pub fn backup_outcome(folder: &str, runs: &[PackageRun]) -> (String, bool) {
    let done = runs.iter().filter(|r| r.done()).count();
    let mut lines = Vec::new();
    if done > 0 {
        lines.push(format!("Backed up {} to {folder}.", crate::addons::plural(done, "driver package")));
    }
    lines.extend(runs.iter().filter(|r| !r.done()).map(|r| format!("Not backed up: {} ({})", r.name, r.reason())));
    (lines.join("\n"), done == 0)
}

// ------------------------------------------------------------------- XML

/// An element of the tool's XML: the part of the format the tool writes.
#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
}

/// `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;` and numeric references.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.find(';').and_then(|end| {
            let name = &rest[1..end];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The attributes of a start tag's text after the element name.
fn parse_attrs(mut s: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start();
        if s.is_empty() {
            return Ok(out);
        }
        let eq = s.find('=').ok_or("an attribute has no value")?;
        let name = s[..eq].trim().to_string();
        let after = s[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or("an attribute value is not quoted")?;
        let end = after[1..].find(quote).ok_or("an attribute value is not closed")?;
        out.push((name, unescape(&after[1..1 + end])));
        s = &after[end + 2..];
    }
}

fn parse_xml(src: &str) -> Result<Node, String> {
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let mut rest = src.trim_start_matches('\u{feff}');
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if stack.is_empty() && rest.trim().is_empty() {
                break;
            }
            return Err(if stack.is_empty() { "the text is not XML".into() } else { "the document ends early".into() });
        };
        let (text, tail) = rest.split_at(lt);
        match stack.last_mut() {
            Some(top) => top.text.push_str(&unescape(text)),
            None if !text.trim().is_empty() => return Err("the text is not XML".into()),
            None => {}
        }
        rest = tail;
        if let Some(r) = rest.strip_prefix("<?") {
            rest = &r[r.find("?>").ok_or("a declaration is not closed")? + 2..];
        } else if let Some(r) = rest.strip_prefix("<!--") {
            rest = &r[r.find("-->").ok_or("a comment is not closed")? + 3..];
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").ok_or("a CDATA section is not closed")?;
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&r[..end]);
            }
            rest = &r[end + 3..];
        } else if let Some(r) = rest.strip_prefix("</") {
            let end = r.find('>').ok_or("an end tag is not closed")?;
            let node = stack.pop().ok_or("an end tag has no start tag")?;
            if node.name != r[..end].trim() {
                return Err(format!("<{}> is closed by </{}>", node.name, r[..end].trim()));
            }
            rest = &r[end + 1..];
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None => root = Some(node),
            }
        } else {
            let r = &rest[1..];
            let end = r.find('>').ok_or("a tag is not closed")?;
            let (inner, empty) = match r[..end].strip_suffix('/') {
                Some(i) => (i, true),
                None => (&r[..end], false),
            };
            let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
            let node = Node { name: inner[..name_end].to_string(), attrs: parse_attrs(&inner[name_end..])?, ..Node::default() };
            rest = &r[end + 1..];
            if empty {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            } else {
                stack.push(node);
            }
        }
    }
    if !stack.is_empty() {
        return Err("the document ends early".into());
    }
    root.ok_or_else(|| "the text is not XML".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `pnputil /enum-drivers /devices /format xml` on a real PC, cut down to
    /// 16 packages and with the devices' serial numbers replaced.
    const REAL: &str = include_str!("fixtures/pnputil-enum-drivers.xml");

    fn real() -> Vec<StorePackage> {
        parse_store(REAL).expect("the recorded output parses")
    }

    fn package<'a>(list: &'a [StorePackage], published: &str) -> &'a StorePackage {
        list.iter().find(|p| p.published == published).unwrap_or_else(|| panic!("{published} is in the recording"))
    }

    fn flags_of(list: &[StorePackage], published: &str) -> Flags {
        let at = list.iter().position(|p| p.published == published).expect("in the recording");
        classify(list)[at]
    }

    #[test]
    fn the_recorded_output_reads_every_column_of_a_package() {
        let list = real();
        assert_eq!(list.len(), 16);
        let p = package(&list, "oem50.inf");
        assert_eq!(p.original, "axusbeth.inf");
        assert_eq!(p.provider, "ASIX");
        assert_eq!(p.class, "Net");
        assert_eq!(p.version, "4.20.2.0");
        assert_eq!(p.date, "2026-10-12");
        assert_eq!(p.signer, "Microsoft Windows Hardware Compatibility Publisher");
        assert_eq!(p.devices, vec![StoreDevice { instance_id: r"USB\VID_0B95&PID_1790\0".into(), name: "ASIX USB to Gigabit Ethernet Family Adapter".into() }]);
    }

    #[test]
    fn a_package_without_devices_has_none_and_one_with_several_has_them_all() {
        let list = real();
        assert!(package(&list, "oem185.inf").devices.is_empty());
        assert_eq!(package(&list, "oem176.inf").devices.len(), 4);
    }

    #[test]
    fn an_old_package_has_a_newer_one_of_the_same_inf_and_provider() {
        let list = real();
        assert!(flags_of(&list, "oem81.inf").old, "amdgpio2 2019 has 2024");
        assert!(!flags_of(&list, "oem130.inf").old, "the newest is not old");
        assert!(!flags_of(&list, "oem146.inf").old, "a package alone is not old");
        // The older version is the one in use here: still flagged old.
        let f = flags_of(&list, "oem50.inf");
        assert!(f.old && f.in_use);
        assert!(!flags_of(&list, "oem185.inf").old);
    }

    #[test]
    fn the_same_version_is_told_apart_by_date() {
        let list = real();
        // Three nvhda.inf packages share 1.4.5.7 and a newer version exists: all old.
        for p in ["oem111.inf", "oem63.inf", "oem179.inf"] {
            assert!(flags_of(&list, p).old, "{p}");
        }
        assert!(!flags_of(&list, "oem195.inf").old);

        let pkg = |name: &str, version: &str, date: &str| StorePackage { published: name.into(), original: "x.inf".into(), provider: "P".into(), version: version.into(), date: date.into(), ..Default::default() };
        let tied = [pkg("oem1.inf", "1.0.0.0", "2024-01-01"), pkg("oem2.inf", "1.0.0.0", "2025-01-01")];
        assert_eq!(classify(&tied).iter().map(|f| f.old).collect::<Vec<_>>(), [true, false], "the later date wins a tie");
        let twins = [pkg("oem1.inf", "1.0.0.0", "2024-01-01"), pkg("oem2.inf", "1.0.0.0", "2024-01-01")];
        assert!(classify(&twins).iter().all(|f| !f.old), "identical packages are not old");
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        let pkg = |name: &str, version: &str| StorePackage { published: name.into(), original: "x.inf".into(), provider: "P".into(), version: version.into(), date: "2024-01-01".into(), ..Default::default() };
        let list = [pkg("oem1.inf", "10.0.0.0"), pkg("oem2.inf", "9.0.0.0"), pkg("oem3.inf", "10.0")];
        let f = classify(&list);
        assert_eq!((f[0].old, f[1].old, f[2].old), (false, true, false), "10.0 equals 10.0.0.0");
    }

    #[test]
    fn another_provider_or_inf_is_another_family() {
        let pkg = |name: &str, inf: &str, prov: &str, version: &str| StorePackage { published: name.into(), original: inf.into(), provider: prov.into(), version: version.into(), ..Default::default() };
        let list = [pkg("oem1.inf", "a.inf", "Acme", "1.0.0.0"), pkg("oem2.inf", "a.inf", "Other", "2.0.0.0"), pkg("oem3.inf", "b.inf", "Acme", "2.0.0.0"), pkg("oem4.inf", "A.INF", "ACME", "3.0.0.0")];
        let old: Vec<bool> = classify(&list).iter().map(|f| f.old).collect();
        assert_eq!(old, [true, false, false, false], "case does not matter; only a.inf from Acme has a newer one");
    }

    #[test]
    fn in_use_means_at_least_one_device() {
        let list = real();
        assert!(flags_of(&list, "oem146.inf").in_use);
        assert!(!flags_of(&list, "oem185.inf").in_use);
        assert!(flags_of(&list, "oem176.inf").in_use);
    }

    #[test]
    fn microsoft_packages_are_flagged_inbox() {
        let list = real();
        for (name, inbox) in [("oem10.inf", true), ("oem172.inf", true), ("oem191.inf", true), ("oem101.inf", true), ("oem130.inf", false), ("oem178.inf", false)] {
            assert_eq!(flags_of(&list, name).inbox, inbox, "{name}");
        }
        assert_eq!(Flags { old: true, in_use: true, inbox: true }.words(), ["Old", "In use", "Inbox"]);
        assert!(Flags::default().words().is_empty());
    }

    #[test]
    fn the_figures_count_the_store_as_a_whole() {
        let mut list = real();
        for (i, p) in list.iter_mut().enumerate() {
            p.size = Some(1000 * (i as u64 + 1));
        }
        let flags = classify(&list);
        let f = StoreFigures::of(&list, &flags);
        assert_eq!(f.packages, 16);
        assert_eq!(f.third_party, 12, "four Microsoft packages are inbox");
        // Old, not in use, not inbox: oem81, oem111, oem63, oem179, oem29.
        assert_eq!(f.old, 5);
        assert_eq!(f.bytes, Some(1000 * (16 * 17 / 2)));
        let freeable: u64 = ["oem81.inf", "oem111.inf", "oem63.inf", "oem179.inf", "oem29.inf"].iter().map(|n| package(&list, n).size.unwrap()).sum();
        assert_eq!(f.freeable, Some(freeable));
    }

    #[test]
    fn unknown_sizes_are_not_zero() {
        let list = real();
        let f = StoreFigures::of(&list, &classify(&list));
        assert_eq!((f.bytes, f.freeable), (None, None), "no folder was measured");
    }

    #[test]
    fn the_list_is_by_inf_newest_first_and_follows_the_filter() {
        let packages: Arc<[StorePackage]> = real().into();
        let all = StoreList::build(&packages, "");
        assert_eq!(all.rows.len(), 16);
        let names = |l: &StoreList| l.rows.iter().map(|&i| packages[i].published.clone()).collect::<Vec<_>>();
        let nv = names(&StoreList::build(&packages, "nvhda"));
        assert_eq!(nv, ["oem195.inf", "oem111.inf", "oem63.inf", "oem179.inf"], "newest first, ties by date");
        // A device name, a provider, a class and a published name all match.
        assert_eq!(names(&StoreList::build(&packages, " GIGABIT ")), ["oem50.inf"]);
        assert_eq!(names(&StoreList::build(&packages, "oem10.inf")), ["oem10.inf"]);
        assert!(!names(&StoreList::build(&packages, "steelseries")).is_empty());
        assert!(names(&StoreList::build(&packages, "no such package")).is_empty());
        // Figures describe the store, not the filter.
        assert_eq!(StoreList::build(&packages, "nvhda").figures.packages, 16);
    }

    #[test]
    fn a_list_is_reused_for_the_same_read_and_filter_only() {
        let packages: Arc<[StorePackage]> = real().into();
        let list = StoreList::build(&packages, "nv");
        assert!(list.is_for(&packages, " NV "));
        assert!(!list.is_for(&packages, "amd"));
        let again: Arc<[StorePackage]> = real().into();
        assert!(!list.is_for(&again, "nv"), "a new read");
    }

    #[test]
    fn text_that_is_not_xml_is_an_error_not_an_empty_store() {
        let usage = "PNPUTIL [/add-driver <...> | /delete-driver <...> |\n         /export-driver <...>]\n";
        assert!(parse_store(usage).unwrap_err().contains("did not return the driver store as XML"));
        assert!(parse_store("").is_err());
        assert!(parse_store("<PnpUtil><Driver DriverName=\"oem1.inf\"></PnpUtil>").is_err(), "mismatched tags");
        assert!(parse_store("<Other/>").is_err(), "another document");
    }

    #[test]
    fn an_empty_store_is_an_empty_list() {
        assert_eq!(parse_store(r#"<?xml version="1.0" encoding="utf-8"?><PnpUtil Version="10"/>"#).unwrap(), Vec::new());
        assert_eq!(parse_store("<PnpUtil></PnpUtil>").unwrap(), Vec::new());
    }

    #[test]
    fn entities_comments_and_cdata_are_read() {
        let xml = "<PnpUtil><!-- c --><Driver DriverName=\"oem1.inf\"><ProviderName>A &amp; B &lt;C&gt; &#65;&#x42;</ProviderName><SignerName><![CDATA[x <y>]]></SignerName></Driver></PnpUtil>";
        let p = &parse_store(xml).unwrap()[0];
        assert_eq!(p.provider, "A & B <C> AB");
        assert_eq!(p.signer, "x <y>");
    }

    #[test]
    fn the_date_is_month_day_year_and_never_guessed_wrong() {
        assert_eq!(split_version("10/28/2024 24.30.0.2"), ("2024-10-28".into(), "24.30.0.2".into()));
        assert_eq!(split_version("28/10/2024 1.0"), ("2024-10-28".into(), "1.0".into()), "a first number above 12 is a day");
        assert_eq!(split_version("garbage"), (String::new(), "garbage".into()));
        assert_eq!(split_version("13/40/2024 1.0"), (String::new(), "13/40/2024 1.0".into()));
        assert_eq!(split_version(""), (String::new(), String::new()));
    }
}

#[cfg(test)]
mod removal_tests {
    use super::*;

    fn pkg(published: &str, original: &str, provider: &str, version: &str, devices: usize) -> StorePackage {
        StorePackage {
            published: published.into(),
            original: original.into(),
            provider: provider.into(),
            version: version.into(),
            devices: (0..devices).map(|i| StoreDevice { instance_id: format!("D{i}"), name: format!("Device {i}") }).collect(),
            size: Some(1_048_576),
            ..Default::default()
        }
    }

    /// oem1 old, oem2 its newer one, oem3 old but in use, oem4 inbox old, oem5 newest and in use.
    fn store() -> Vec<StorePackage> {
        vec![
            pkg("oem1.inf", "a.inf", "Acme", "1.0.0.0", 0),
            pkg("oem2.inf", "a.inf", "Acme", "3.0.0.0", 0),
            pkg("oem3.inf", "b.inf", "Beta", "1.0.0.0", 1),
            pkg("oem4.inf", "c.inf", "Microsoft", "1.0.0.0", 0),
            pkg("oem5.inf", "b.inf", "Beta", "2.0.0.0", 2),
            pkg("oem6.inf", "c.inf", "Microsoft", "2.0.0.0", 0),
        ]
    }

    fn names(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn remove_old_picks_only_old_packages_that_no_device_uses_and_are_not_inbox() {
        assert_eq!(old_removable(&store()), ["oem1.inf"], "oem3 is in use, oem4 is inbox, the rest are newest");
        assert!(old_removable(&[]).is_empty());
    }

    #[test]
    fn back_up_everything_means_every_third_party_package() {
        assert_eq!(third_party(&store()), ["oem1.inf", "oem2.inf", "oem3.inf", "oem5.inf"]);
    }

    #[test]
    fn ticking_toggles_a_package_and_never_ticks_an_inbox_one() {
        let s = store();
        let mut sel = Selection::default();
        sel.toggle(&s[0]);
        sel.toggle(&s[3]);
        assert_eq!(sel.names(), ["oem1.inf"], "the inbox package gets no tick");
        sel.toggle(&s[1]);
        assert_eq!((sel.len(), sel.contains("oem2.inf")), (2, true));
        sel.toggle(&s[0]);
        assert_eq!(sel.names(), ["oem2.inf"]);
        sel.keep_only(&s[..1]);
        assert!(sel.is_empty(), "a package a new read no longer holds is forgotten");
    }

    #[test]
    fn a_plain_removal_refuses_in_use_inbox_and_unknown_packages_and_runs_the_rest() {
        let plan = plan_removal(&store(), &names(&["oem1.inf", "oem3.inf", "oem4.inf", "oem9.inf", "oem1.inf"]), false);
        assert_eq!(plan.run, ["oem1.inf"], "listed twice, run once");
        assert_eq!(
            plan.refused,
            [
                ("oem3.inf".to_string(), "a device uses it".to_string()),
                ("oem4.inf".to_string(), "it ships with Windows".to_string()),
                ("oem9.inf".to_string(), "it is not in the driver store".to_string())
            ]
        );
    }

    #[test]
    fn force_counts_for_one_package_only_and_never_for_an_inbox_one() {
        assert_eq!(plan_removal(&store(), &names(&["oem3.inf"]), true).run, ["oem3.inf"]);
        let many = plan_removal(&store(), &names(&["oem3.inf", "oem1.inf"]), true);
        assert_eq!((many.run, many.refused.len()), (names(&["oem1.inf"]), 1), "two packages: no force");
        assert!(plan_removal(&store(), &names(&["oem4.inf"]), true).run.is_empty());
    }

    fn run(name: &str, code: i64, output: &str) -> PackageRun {
        PackageRun { name: name.into(), code, output: output.into() }
    }

    #[test]
    fn the_outcome_says_how_many_were_removed_how_much_was_freed_and_what_stayed() {
        let size = |n: &str| (n != "oem2.inf").then_some(3 * 1_048_576);
        let out = removal_outcome(&[("oem3.inf".into(), IN_USE_REASON.into())], &[run("oem1.inf", 0, ""), run("oem2.inf", 0, "")], size);
        assert_eq!(out.message, ("Removed 2 packages, freed 3.0 MB.\nNot removed: oem3.inf (a device uses it)".to_string(), false));
        assert_eq!((out.removed, out.reboot), (2, false));
    }

    #[test]
    fn nothing_removed_is_an_error_and_a_restart_is_reported() {
        let out = removal_outcome(&[], &[run("oem1.inf", 1, "Microsoft PnP Utility\n\nDeleting driver package: oem1.inf\nFailed to delete driver package: The package is in use.\n")], |_| None);
        assert_eq!(out.message, ("Not removed: oem1.inf (The package is in use)".to_string(), true));
        let out = removal_outcome(&[], &[run("oem1.inf", 3010, "")], |_| None);
        assert_eq!((out.message.0.as_str(), out.reboot), ("Removed 1 package.", true), "no size known: nothing is said about it");
        let out = removal_outcome(&[], &[run("oem1.inf", 5, "")], |_| None);
        assert_eq!(out.message.0, "Not removed: oem1.inf (pnputil returned code 5)");
    }

    #[test]
    fn a_job_result_is_read_into_one_run_per_package() {
        let v = serde_json::json!({"ok": true, "items": [{"name": "oem1.inf", "code": 0, "output": "x"}, {"name": "oem2.inf", "code": 1, "output": "y"}]});
        assert_eq!(parse_runs(&v).unwrap(), [run("oem1.inf", 0, "x"), run("oem2.inf", 1, "y")]);
        assert_eq!(parse_runs(&serde_json::json!({"ok": false, "error": "no folder"})), Err("no folder".to_string()));
    }

    #[test]
    fn the_backup_goes_into_a_dated_subfolder_of_the_picked_folder() {
        assert_eq!(backup_folder(r"D:\Backups", (2026, 11, 3)), r"D:\Backups\Drivers 2026-11-03");
        assert_eq!(backup_folder(r"D:\Backups\", (2026, 11, 23)), r"D:\Backups\Drivers 2026-11-23");
    }

    #[test]
    fn the_backup_outcome_counts_what_was_exported_and_lists_the_failures() {
        let folder = r"D:\B\Drivers 2026-11-23";
        assert_eq!(backup_outcome(folder, &[run("oem1.inf", 0, ""), run("oem2.inf", 0, "")]), (format!("Backed up 2 driver packages to {folder}."), false));
        assert_eq!(backup_outcome(folder, &[run("oem1.inf", 0, "")]).0, format!("Backed up 1 driver package to {folder}."));
        let (text, error) = backup_outcome(folder, &[run("oem1.inf", 0, ""), run("oem2.inf", 2, "Failed to export driver package: The system cannot find the file specified.")]);
        assert_eq!(text, format!("Backed up 1 driver package to {folder}.\nNot backed up: oem2.inf (The system cannot find the file specified)"));
        assert!(!error);
        assert!(backup_outcome(folder, &[run("oem2.inf", 2, "")]).1, "all failed: an error");
    }
}
