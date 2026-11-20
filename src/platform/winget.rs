//! What every add-on needs to read winget: its tables (`list` and `search`),
//! which installed app each package is, the lines of its output worth
//! showing, its exit codes in words and their outcome, and the summary
//! texts after a batch of updates or uninstalls.
//!
//! Tables are read by display column, as winget pads them: a wide character
//! (CJK, full-width) takes two columns. Column starts come from the header
//! line above the dashes, so the reading works whatever language winget
//! prints in.

/// A row of `winget list`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WingetRow {
    pub name: String,
    pub id: String,
    pub version: String,
    pub available: String,
    pub source: String,
}

/// A row of `winget search`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchRow {
    pub name: String,
    /// The id as printed; when `truncated` it ends with "…" and is not the
    /// package's real id.
    pub id: String,
    pub version: String,
    /// Why the row matched when it was not by name or id ("Tag: vlc").
    pub matched: String,
    pub source: String,
    /// winget shortened the id to fit the column: never install by it.
    pub truncated: bool,
}

/// What winget prints at the end of a shortened cell.
pub const ELLIPSIS: char = '\u{2026}';

/// Parses `winget list`.
pub fn parse_winget_table(text: &str) -> Vec<WingetRow> {
    let mut out = Vec::new();
    for c in table_cells(text) {
        let mut row = WingetRow { name: c[0].clone(), id: c[1].clone(), version: c[2].clone(), ..Default::default() };
        match c.len() {
            3 => {}
            4 => {
                if is_source(&c[3]) {
                    row.source = c[3].clone();
                } else {
                    row.available = c[3].clone();
                }
            }
            _ => {
                row.available = c[3].clone();
                row.source = c[4].clone();
            }
        }
        if is_id(&row.id) {
            out.push(row);
        }
    }
    out
}

/// Parses `winget search`: Name, Id, Version, then Match when some row did
/// not match by name or id, then Source unless one source was asked for.
pub fn parse_search_table(text: &str) -> Vec<SearchRow> {
    let mut out = Vec::new();
    for c in table_cells(text) {
        let mut row = SearchRow { name: c[0].clone(), id: c[1].clone(), version: c[2].clone(), ..Default::default() };
        match c.len() {
            3 => {}
            4 => {
                if c[3].contains(": ") {
                    row.matched = c[3].clone();
                } else {
                    row.source = c[3].clone();
                }
            }
            _ => {
                row.matched = c[3].clone();
                row.source = c[4].clone();
            }
        }
        row.truncated = row.id.ends_with(ELLIPSIS);
        if is_id(&row.id) {
            out.push(row);
        }
    }
    out
}

/// Parses `winget source list`: the names of the catalogs registered on
/// this PC ("winget", "msstore"), first column of each row.
pub fn parse_source_list(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(|l| l.rsplit('\r').next().unwrap_or(l)).collect();
    let Some(dash) = lines.iter().position(|l| l.len() > 10 && l.trim().chars().all(|c| c == '-')) else {
        return Vec::new();
    };
    lines[dash + 1..].iter().take_while(|l| !l.trim().is_empty()).filter_map(|l| l.split_whitespace().next()).map(str::to_string).collect()
}

fn is_source(s: &str) -> bool {
    s == "winget" || s == "msstore"
}

fn is_id(s: &str) -> bool {
    !s.is_empty() && !s.contains(' ')
}

/// The cells of each row of winget's table (always at least three).
fn table_cells(text: &str) -> Vec<Vec<String>> {
    let lines: Vec<&str> = text.lines().map(|l| l.rsplit('\r').next().unwrap_or(l)).collect();
    let Some(dash) = lines.iter().position(|l| l.len() > 10 && l.trim().chars().all(|c| c == '-')) else {
        return Vec::new();
    };
    if dash == 0 {
        return Vec::new();
    }
    let header = columns(lines[dash - 1]);
    let starts_after = |gap: usize| -> Vec<usize> {
        (0..header.len()).filter(|&i| header[i].1 != ' ' && (i == 0 || (i >= gap && (1..=gap).all(|g| header[i - g].1 == ' ')))).map(|i| header[i].0).collect()
    };
    let mut starts = starts_after(1);
    // A header made of several words: keep only starts after two spaces.
    if starts.len() > 5 {
        starts = starts_after(2);
    }
    if starts.len() < 3 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for l in &lines[dash + 1..] {
        if l.trim().is_empty() {
            break;
        }
        let mut cells = vec![String::new(); starts.len()];
        for (col, ch) in columns(l) {
            let k = starts.iter().rposition(|&s| s <= col).unwrap_or(0);
            cells[k].push(ch);
        }
        out.push(cells.into_iter().map(|c| c.trim().to_string()).collect());
    }
    out
}

/// Each character of a line with the display column it starts at.
fn columns(line: &str) -> Vec<(usize, char)> {
    let mut col = 0;
    line.chars()
        .map(|c| {
            let at = col;
            col += char_width(c);
            (at, c)
        })
        .collect()
}

/// How many columns a character takes in the console: two for East Asian
/// wide and full-width characters, one otherwise.
fn char_width(c: char) -> usize {
    let u = c as u32;
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD);
    if wide {
        2
    } else {
        1
    }
}

// ------------------------------------------------------------- matching

/// An installed app as the matching sees it.
#[derive(Clone, Copy, Debug)]
pub struct InstalledEntry<'a> {
    /// "HKLM\<key>", "HKLM32\<key>", "HKCU\<key>" or "MSIX\<full name>".
    pub key: &'a str,
    pub name: &'a str,
    pub version: &'a str,
}

/// For each row of `winget list`, the app it is, if any. A row whose id is
/// a registry entry (`ARP\…`) or a Store package (`MSIX\…`) is found by
/// key; any other row by name (the same version first), and claims that
/// app so another row with the same name goes to the next one.
pub fn match_rows(apps: &[InstalledEntry], rows: &[WingetRow]) -> Vec<Option<usize>> {
    let mut claimed = vec![false; apps.len()];
    let by_key = |key: String| apps.iter().position(|a| a.key.to_lowercase() == key);
    rows.iter()
        .map(|r| {
            if let Some(rest) = r.id.strip_prefix("ARP\\") {
                // ARP\Machine\X64\<key>, ARP\Machine\X86\<key>, ARP\User\X64\<key>
                let mut it = rest.splitn(3, '\\');
                let (scope, arch, sub) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
                let hive = match (scope, arch) {
                    ("User", _) => "HKCU",
                    (_, "X86") => "HKLM32",
                    _ => "HKLM",
                };
                by_key(format!("{hive}\\{sub}").to_lowercase())
            } else if let Some(full) = r.id.strip_prefix("MSIX\\") {
                by_key(format!("MSIX\\{full}").to_lowercase())
            } else {
                let name = r.name.to_lowercase();
                let by_name: Vec<usize> = (0..apps.len()).filter(|&i| apps[i].name.trim().to_lowercase() == name && !claimed[i]).collect();
                let i = by_name.iter().copied().find(|&i| apps[i].version == r.version).or(by_name.first().copied());
                if let Some(i) = i {
                    claimed[i] = true;
                }
                i
            }
        })
        .collect()
}

// --------------------------------------------------------------- output

/// The last `n` lines of winget's output worth showing: no blank lines and
/// no spinner or progress bar.
pub fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().map(|l| l.rsplit('\r').next().unwrap_or(l).trim_end()).filter(|l| !l.trim().is_empty() && !is_progress(l)).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// winget's spinner and progress bar lines.
fn is_progress(l: &str) -> bool {
    let t = l.trim();
    t.len() <= 2 || t.contains('\u{2588}') || t.contains('\u{2592}')
}

// --------------------------------------------------------- batch summaries

/// "A", "A and B", "A, B and C".
pub fn join_names(v: &[String]) -> String {
    match v.len() {
        0 => String::new(),
        1 => v[0].clone(),
        _ => format!("{} and {}", v[..v.len() - 1].join(", "), v[v.len() - 1]),
    }
}

/// The message after a batch of updates: what was updated, what was not.
pub fn update_summary(ok: &[String], failed: &[String]) -> (String, bool) {
    if failed.is_empty() {
        (format!("Updated {}.", join_names(ok)), false)
    } else if ok.is_empty() {
        (format!("Not updated: {}.", failed.join("; ")), true)
    } else {
        (format!("Updated {}. Not updated: {}.", join_names(ok), failed.join("; ")), true)
    }
}

/// The message after uninstalling `n` apps: how many went, and why the
/// others did not.
pub fn uninstall_summary(n: usize, gone: &[String], notes: &[String]) -> (String, bool) {
    match (n, gone.len()) {
        (1, 1) => (format!("{} was uninstalled.", gone[0]), false),
        (1, _) => (notes.join(" "), true),
        (_, g) if g == n => (format!("{g} of {n} applications were uninstalled."), false),
        (_, g) => (format!("{g} of {n} applications were uninstalled. {}", notes.join(" ")), true),
    }
}

// ----------------------------------------------------------- exit codes

/// winget's code for "no package found" (a search, list or check found
/// nothing).
pub const NO_PACKAGE_FOUND: i32 = 0x8A15_0014_u32 as i32;

/// winget's code for "running uninstall command failed": the package's own
/// uninstaller failed. A machine-wide MSI that winget runs silently from a
/// process without administrator rights cannot ask for them and ends with
/// 1603 (winget's log: "MsiExec uninstaller failed: 1603").
pub const UNINSTALL_COMMAND_FAILED: i32 = 0x8A15_0030_u32 as i32;

/// The uninstall registry entry `winget list --id … --exact --details`
/// names for an installed package: the `ARP\…` value of its "Local
/// Identifier" line (found by its value, whatever the label's language).
pub fn parse_local_id(text: &str) -> Option<String> {
    text.lines().filter_map(|l| l.split_once(": ")).map(|(_, v)| v.trim()).find(|v| v.starts_with("ARP\\")).map(String::from)
}

/// What an exit code of winget (or of the MSI installer it ran) means for
/// the package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitClass {
    Installed,
    /// Installed; Windows must restart to finish.
    InstalledRestart,
    /// Nothing was done, for this reason.
    Skipped(&'static str),
    Cancelled,
    Failed,
}

/// The class of an exit code.
pub fn classify(code: i32) -> ExitClass {
    match code as u32 {
        0 => ExitClass::Installed,
        1641 | 3010 | 0x8A15_0109 | 0x8A15_010B => ExitClass::InstalledRestart,
        1602 | 0x8A15_0005 | 0x8A15_006A | 0x8A15_0077 | 0x8A15_010C => ExitClass::Cancelled,
        0x8A15_002B | 0x8A15_004F | 0x8A15_0050 | 0x8A15_0061 | 0x8A15_0068 | 0x8A15_0089 | 0x8A15_008E | 0x8A15_010D | 0x8A15_010E => {
            ExitClass::Skipped(words(code).unwrap_or_default())
        }
        _ => ExitClass::Failed,
    }
}

/// An exit code in words, with the raw code beside them so it can be
/// looked up: "another installation is in progress (0x8A150102)". A code
/// nobody documents is shown as it is: in hex when it is winget's
/// ("winget code 0x8A150FFF"), in decimal otherwise ("exit code 1603").
pub fn describe(code: i32) -> String {
    let hex = format!("0x{:08X}", code as u32);
    match words(code) {
        Some(w) if is_winget_code(code) => format!("{w} ({hex})"),
        Some(w) => format!("{w} ({code})"),
        None if is_winget_code(code) => format!("winget code {hex}"),
        None => format!("exit code {code}"),
    }
}

/// winget's own codes: HRESULTs of its facility, 0x8A15xxxx (errors) and
/// 0x0A15xxxx (statuses).
fn is_winget_code(code: i32) -> bool {
    (code as u32 & 0x7FFF_0000) == 0x0A15_0000
}

/// An exit code in words: the MSI installer's codes, plainer words for the
/// codes an update meets most, then winget's own table.
fn words(code: i32) -> Option<&'static str> {
    let plain = match code as u32 {
        0 => "done",
        1602 => "the installation was cancelled",
        1618 => "another installation is in progress",
        1641 => "the installer is restarting Windows to finish",
        3010 => "restart Windows to finish",
        0x8A15_0010 => "no installer of the package fits this PC",
        0x8A15_0014 => "no package found",
        0x8A15_002B => "no applicable update found",
        0x8A15_0061 => "a version of the package is already installed",
        0x8A15_0101 => "the app is running; close it and try again",
        0x8A15_0102 => "another installation is in progress",
        0x8A15_0109 => "restart Windows to finish",
        0x8A15_010C => "the installer was cancelled",
        _ => "",
    };
    if !plain.is_empty() {
        return Some(plain);
    }
    CODES.iter().find(|(c, _)| *c == code as u32).map(|(_, w)| *w)
}

/// winget's return codes and their descriptions, as published for
/// winget-cli v1.29.380 (microsoft/winget-cli, tag v1.29.380,
/// doc/windows/package-manager/winget/returnCodes.md). Descriptions keep
/// winget's wording without the final period, starting lower case unless
/// they start with an acronym ("ICU").
const CODES: &[(u32, &str)] = &[
    (0x8A15_0001, "internal error"),
    (0x8A15_0002, "invalid command line arguments"),
    (0x8A15_0003, "executing command failed"),
    (0x8A15_0004, "opening manifest failed"),
    (0x8A15_0005, "cancellation signal received"),
    (0x8A15_0006, "running ShellExecute failed"),
    (0x8A15_0007, "cannot process manifest. The manifest version is higher than supported. Please update the client"),
    (0x8A15_0008, "downloading installer failed"),
    (0x8A15_0009, "cannot write to index; it is a higher schema version"),
    (0x8A15_000A, "the index is corrupt"),
    (0x8A15_000B, "the configured source information is corrupt"),
    (0x8A15_000C, "the source name is already configured"),
    (0x8A15_000D, "the source type is invalid"),
    (0x8A15_000E, "the MSIX file is a bundle, not a package"),
    (0x8A15_000F, "data required by the source is missing"),
    (0x8A15_0010, "none of the installers are applicable for the current system"),
    (0x8A15_0011, "the installer file's hash does not match the manifest"),
    (0x8A15_0012, "the source name does not exist"),
    (0x8A15_0013, "the source location is already configured under another name"),
    (0x8A15_0014, "no packages found"),
    (0x8A15_0015, "no sources are configured"),
    (0x8A15_0016, "multiple packages found matching the criteria"),
    (0x8A15_0017, "no manifest found matching the criteria"),
    (0x8A15_0018, "failed to get Public folder from source package"),
    (0x8A15_0019, "command requires administrator privileges to run"),
    (0x8A15_001A, "the source location is not secure"),
    (0x8A15_001B, "the Microsoft Store client is blocked by policy"),
    (0x8A15_001C, "the Microsoft Store app is blocked by policy"),
    (0x8A15_001D, "the feature is currently under development. It can be enabled using winget settings"),
    (0x8A15_001E, "failed to install the Microsoft Store app"),
    (0x8A15_001F, "failed to perform auto complete"),
    (0x8A15_0020, "failed to initialize YAML parser"),
    (0x8A15_0021, "encountered an invalid YAML key"),
    (0x8A15_0022, "encountered a duplicate YAML key"),
    (0x8A15_0023, "invalid YAML operation"),
    (0x8A15_0024, "failed to build YAML doc"),
    (0x8A15_0025, "invalid YAML emitter state"),
    (0x8A15_0026, "invalid YAML data"),
    (0x8A15_0027, "libYAML error"),
    (0x8A15_0028, "manifest validation succeeded with warning"),
    (0x8A15_0029, "manifest validation failed"),
    (0x8A15_002A, "manifest is invalid"),
    (0x8A15_002B, "no applicable update found"),
    (0x8A15_002C, "winget upgrade --all completed with failures"),
    (0x8A15_002D, "installer failed security check"),
    (0x8A15_002E, "download size does not match expected content length"),
    (0x8A15_002F, "uninstall command not found"),
    (0x8A15_0030, "running uninstall command failed"),
    (0x8A15_0031, "ICU break iterator error"),
    (0x8A15_0032, "ICU casemap error"),
    (0x8A15_0033, "ICU regex error"),
    (0x8A15_0034, "failed to install one or more imported packages"),
    (0x8A15_0035, "could not find one or more requested packages"),
    (0x8A15_0036, "json file is invalid"),
    (0x8A15_0037, "the source location is not remote"),
    (0x8A15_0038, "the configured rest source is not supported"),
    (0x8A15_0039, "invalid data returned by rest source"),
    (0x8A15_003A, "operation is blocked by Group Policy"),
    (0x8A15_003B, "rest API internal error"),
    (0x8A15_003C, "invalid rest source url"),
    (0x8A15_003D, "unsupported MIME type returned by rest API"),
    (0x8A15_003E, "invalid rest source contract version"),
    (0x8A15_003F, "the source data is corrupted or tampered"),
    (0x8A15_0040, "error reading from the stream"),
    (0x8A15_0041, "package agreements were not agreed to"),
    (0x8A15_0042, "error reading input in prompt"),
    (0x8A15_0043, "the search request is not supported by one or more sources"),
    (0x8A15_0044, "the rest API endpoint is not found"),
    (0x8A15_0045, "failed to open the source"),
    (0x8A15_0046, "source agreements were not agreed to"),
    (0x8A15_0047, "header size exceeds the allowable limit of 1024 characters. Please reduce the size and try again"),
    (0x8A15_0048, "missing resource file"),
    (0x8A15_0049, "running MSI install failed"),
    (0x8A15_004A, "arguments for msiexec are invalid"),
    (0x8A15_004B, "failed to open one or more sources"),
    (0x8A15_004C, "failed to validate dependencies"),
    (0x8A15_004D, "one or more package is missing"),
    (0x8A15_004E, "invalid table column"),
    (0x8A15_004F, "the upgrade version is not newer than the installed version"),
    (0x8A15_0050, "upgrade version is unknown and override is not specified"),
    (0x8A15_0051, "ICU conversion error"),
    (0x8A15_0052, "failed to install portable package"),
    (0x8A15_0053, "volume does not support reparse points"),
    (0x8A15_0054, "portable package from a different source already exists"),
    (0x8A15_0055, "unable to create symlink, path points to a directory"),
    (0x8A15_0056, "the installer cannot be run from an administrator context"),
    (0x8A15_0057, "failed to uninstall portable package"),
    (0x8A15_0058, "failed to validate DisplayVersion values against index"),
    (0x8A15_0059, "one or more arguments are not supported"),
    (0x8A15_005A, "embedded null characters are disallowed for SQLite"),
    (0x8A15_005B, "failed to find the nested installer in the archive"),
    (0x8A15_005C, "failed to extract archive"),
    (0x8A15_005D, "invalid relative file path to nested installer provided"),
    (0x8A15_005E, "the server certificate did not match any of the expected values"),
    (0x8A15_005F, "install location must be provided"),
    (0x8A15_0060, "archive malware scan failed"),
    (0x8A15_0061, "found at least one version of the package installed"),
    (0x8A15_0062, "a pin already exists for the package"),
    (0x8A15_0063, "there is no pin for the package"),
    (0x8A15_0064, "unable to open the pin database"),
    (0x8A15_0065, "one or more applications failed to install"),
    (0x8A15_0066, "one or more applications failed to uninstall"),
    (0x8A15_0067, "one or more queries did not return exactly one match"),
    (0x8A15_0068, "the package has a pin that prevents upgrade"),
    (0x8A15_0069, "the package currently installed is the stub package"),
    (0x8A15_006A, "application shutdown signal received"),
    (0x8A15_006B, "failed to download package dependencies"),
    (0x8A15_006C, "failed to download package. Download for offline installation is prohibited"),
    (0x8A15_006D, "a required service is busy or unavailable. Try again later"),
    (0x8A15_006E, "the guid provided does not correspond to a valid resume state"),
    (0x8A15_006F, "the current client version did not match the client version of the saved state"),
    (0x8A15_0070, "the resume state data is invalid"),
    (0x8A15_0071, "unable to open the checkpoint database"),
    (0x8A15_0072, "exceeded max resume limit"),
    (0x8A15_0073, "invalid authentication info"),
    (0x8A15_0074, "authentication method not supported"),
    (0x8A15_0075, "authentication failed"),
    (0x8A15_0076, "authentication failed. Interactive authentication required"),
    (0x8A15_0077, "authentication failed. User cancelled"),
    (0x8A15_0078, "authentication failed. Authenticated account is not the desired account"),
    (0x8A15_0079, "repair command not found"),
    (0x8A15_007A, "repair operation is not applicable"),
    (0x8A15_007B, "repair operation failed"),
    (0x8A15_007C, "the installer technology in use doesn't support repair"),
    (0x8A15_007D, "the requested operation is not permitted from an administrator context on packages installed within the user scope"),
    (0x8A15_007E, "the SQLite connection was terminated to prevent corruption"),
    (0x8A15_007F, "failed to get Microsoft Store package catalog"),
    (0x8A15_0080, "no applicable Microsoft Store package found from Microsoft Store package catalog"),
    (0x8A15_0081, "failed to get Microsoft Store package download information"),
    (0x8A15_0082, "no applicable Microsoft Store package download information found"),
    (0x8A15_0083, "failed to retrieve Microsoft Store package license"),
    (0x8A15_0084, "the Microsoft Store package does not support download"),
    (0x8A15_0085, "failed to retrieve Microsoft Store package license. The Microsoft Entra Id account does not have the required privilege"),
    (0x8A15_0086, "downloaded zero byte installer; ensure that your network connection is working properly"),
    (0x8A15_0087, "failed installing one or more fonts"),
    (0x8A15_0088, "font file is not supported and cannot be installed"),
    (0x8A15_0089, "font package is already installed"),
    (0x8A15_008A, "font file not found"),
    (0x8A15_008B, "font uninstall failed. The font may not be in a good state. Try uninstalling after a restart"),
    (0x8A15_008C, "font validation failed"),
    (0x8A15_008D, "font rollback failed. The font may not be in a good state. Try uninstalling after a restart"),
    (0x8A15_008E, "an upgrade is available but uses a different install technology than the current installation"),
    (0x8A15_0101, "application is currently running. Exit the application then try again"),
    (0x8A15_0102, "another installation is already in progress. Try again later"),
    (0x8A15_0103, "one or more file is being used. Exit the application then try again"),
    (0x8A15_0104, "this package has a dependency missing from your system"),
    (0x8A15_0105, "there's no more space on your PC. Make space, then try again"),
    (0x8A15_0106, "there's not enough memory available to install. Close other applications then try again"),
    (0x8A15_0107, "this application requires internet connectivity. Connect to a network then try again"),
    (0x8A15_0108, "this application encountered an error during installation. Contact support"),
    (0x8A15_0109, "restart your PC to finish installation"),
    (0x8A15_010A, "installation failed. Restart your PC then try again"),
    (0x8A15_010B, "your PC will restart to finish installation"),
    (0x8A15_010C, "you cancelled the installation"),
    (0x8A15_010D, "another version of this application is already installed"),
    (0x8A15_010E, "a higher version of this application is already installed"),
    (0x8A15_010F, "organization policies are preventing installation. Contact your admin"),
    (0x8A15_0110, "failed to install package dependencies"),
    (0x8A15_0111, "application is currently in use by another application"),
    (0x8A15_0112, "invalid parameter"),
    (0x8A15_0113, "package not supported by the system"),
    (0x8A15_0114, "the installer does not support upgrading an existing package"),
    (0x8A15_0115, "installation failed with a custom installer error"),
    (0x8A15_0201, "the Apps and Features Entry for the package could not be found"),
    (0x0A15_0202, "the install location is not applicable"),
    (0x8A15_0203, "the install location could not be found"),
    (0x8A15_0204, "the hash of the existing file did not match"),
    (0x8A15_0205, "file not found"),
    (0x0A15_0206, "the file was found but the hash was not checked"),
    (0x8A15_0207, "the file could not be accessed"),
    (0x8A15_C001, "the configuration file is invalid"),
    (0x8A15_C002, "the YAML syntax is invalid"),
    (0x8A15_C003, "a configuration field has an invalid type"),
    (0x8A15_C004, "the configuration has an unknown version"),
    (0x8A15_C005, "an error occurred while applying the configuration"),
    (0x8A15_C006, "the configuration contains a duplicate identifier"),
    (0x8A15_C007, "the configuration is missing a dependency"),
    (0x8A15_C008, "the configuration has an unsatisfied dependency"),
    (0x8A15_C009, "an assertion for the configuration unit failed"),
    (0x8A15_C00A, "the configuration was manually skipped"),
    (0x8A15_C00B, "the user declined to continue execution"),
    (0x8A15_C00C, "the dependency graph contains a cycle which cannot be resolved"),
    (0x8A15_C00D, "the configuration has an invalid field value"),
    (0x8A15_C00E, "the configuration is missing a field"),
    (0x8A15_C00F, "some of the configuration units failed while testing their state"),
    (0x8A15_C010, "configuration state was not tested"),
    (0x8A15_C011, "the configuration unit failed getting its properties"),
    (0x8A15_C012, "the specified configuration could not be found"),
    (0x8A15_C013, "parameter cannot be passed across integrity boundary"),
    (0x8A15_C101, "the configuration unit was not installed"),
    (0x8A15_C102, "the configuration unit could not be found"),
    (0x8A15_C103, "multiple matches were found for the configuration unit; specify the module to select the correct one"),
    (0x8A15_C104, "the configuration unit failed while attempting to get the current system state"),
    (0x8A15_C105, "the configuration unit failed while attempting to test the current system state"),
    (0x8A15_C106, "the configuration unit failed while attempting to apply the desired state"),
    (0x8A15_C107, "the module for the configuration unit is available in multiple locations with the same version"),
    (0x8A15_C108, "loading the module for the configuration unit failed"),
    (0x8A15_C109, "the configuration unit returned an unexpected result during execution"),
    (0x8A15_C110, "a unit contains a setting that requires the config root"),
    (0x8A15_C111, "loading the module for the configuration unit failed because it requires administrator privileges to run"),
    (0x8A15_C112, "operation is not supported by the configuration processor"),
    (0x8A15_C113, "the DSC processor hash provided does not match hash of the target file"),
];

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "   - \r   \\ \rName                       Id                               Version   Available Source\n\
-------------------------------------------------------------------------------------------\n\
AMD Chipset Software       ARP\\Machine\\X86\\AMD_Chipse       7.06.24\n\
Antigravity 2.5.0          Google.Antigravity               2.5.0     2.32.1    winget\n\
Canva                      XP8K17RNMM8MTN                   1.119.0   1.128.0   msstore\n\
Xbox TCUI                  MSIX\\Xbox.TCUI_1.24_x64__8       1.24.1\n";

    /// The add-ons' sources (src/addons and below), without their tests.
    fn addon_sources() -> Vec<(std::path::PathBuf, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let production = text.split("#[cfg(test)]\nmod tests").next().unwrap_or_default().split("#[cfg(test)]\r\nmod tests").next().unwrap_or_default().to_string();
                    out.push((path, production));
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        walk(&root.join("addons"), &mut out);
        out.push((root.join("addons.rs"), std::fs::read_to_string(root.join("addons.rs")).unwrap().split("#[cfg(test)]").next().unwrap().to_string()));
        out
    }

    #[test]
    fn no_addon_reads_winget_tables_or_names_its_exit_codes_itself() {
        let sources = addon_sources();
        assert!(sources.len() > 20, "the add-ons' sources were found");
        for (path, text) in sources {
            for (pattern, what) in [("0x8A15", "a winget exit code"), ("0x8a15", "a winget exit code"), ("c == '-')", "winget's table header line")] {
                assert!(!text.contains(pattern), "{} has {what} of its own ({pattern:?}): use platform::winget", path.display());
            }
        }
    }

    #[test]
    fn the_source_list_gives_the_registered_catalogs() {
        let three = "Name    Argument                                      Explicit\r\n\
--------------------------------------------------------------\r\n\
msstore https://storeedgefd.dsx.mp.microsoft.com/v9.0 false\r\n\
winget  https://cdn.winget.microsoft.com/cache        false\r\n";
        assert_eq!(parse_source_list(three), ["msstore", "winget"]);
        let two = "Name   Argument\n----------------------------------------\nwinget https://cdn.winget.microsoft.com/cache\n";
        assert_eq!(parse_source_list(two), ["winget"], "older winget prints no Explicit column");
        assert!(parse_source_list("There are no sources configured.").is_empty());
    }

    #[test]
    fn reads_the_list_table() {
        let rows = parse_winget_table(LIST);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1], WingetRow { name: "Antigravity 2.5.0".into(), id: "Google.Antigravity".into(), version: "2.5.0".into(), available: "2.32.1".into(), source: "winget".into() });
        assert_eq!(rows[2].source, "msstore");
        assert_eq!(rows[0].available, "");
    }

    // `winget search vlc` in English: the Match column appears because some
    // rows matched by tag or moniker; long ids end with "…".
    const SEARCH_EN: &str = "\r   - \r   \\ \rName                           Id                               Version     Match           Source\n\
-------------------------------------------------------------------------------------------------------\n\
VLC media player               VideoLAN.VLC                     3.0.21      Moniker: vlc    winget\n\
VLC Skins Pack                 SomeVendor.VeryLongPackageName.…  1.2         Tag: vlc        winget\n\
VLC UWP                        9NBLGGH4VVNH                     Unknown                     msstore\n";

    #[test]
    fn reads_the_search_table_in_english_with_match_and_truncated_ids() {
        let rows = parse_search_table(SEARCH_EN);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0],
            SearchRow { name: "VLC media player".into(), id: "VideoLAN.VLC".into(), version: "3.0.21".into(), matched: "Moniker: vlc".into(), source: "winget".into(), truncated: false }
        );
        assert_eq!(rows[1].id, "SomeVendor.VeryLongPackageName.\u{2026}");
        assert!(rows[1].truncated);
        assert_eq!(rows[1].matched, "Tag: vlc");
        assert_eq!((rows[2].matched.as_str(), rows[2].source.as_str(), rows[2].truncated), ("", "msstore", false));
    }

    // The same search in Italian, from one source only: no Source column.
    const SEARCH_IT: &str = "Nome                Id                  Versione    Corrispondenza\n\
---------------------------------------------------------------------\n\
VLC media player    VideoLAN.VLC        3.0.21      Moniker: vlc\n\
Lettore VLC Pro     Contoso.VlcPlayerP\u{2026}  2.0         Tag: vlc\n";

    #[test]
    fn reads_the_search_table_in_italian_without_a_source_column() {
        let rows = parse_search_table(SEARCH_IT);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].id.as_str(), rows[0].version.as_str(), rows[0].matched.as_str(), rows[0].source.as_str()), ("VideoLAN.VLC", "3.0.21", "Moniker: vlc", ""));
        assert_eq!(rows[1].name, "Lettore VLC Pro");
        assert!(rows[1].truncated);
    }

    #[test]
    fn a_search_without_a_match_column_reads_the_source() {
        let text = "Nome             ID              Versione  Origine\n\
---------------------------------------------------\n\
7-Zip            7zip.7zip       24.09     winget\n";
        let rows = parse_search_table(text);
        assert_eq!(rows, vec![SearchRow { name: "7-Zip".into(), id: "7zip.7zip".into(), version: "24.09".into(), source: "winget".into(), ..Default::default() }]);
    }

    #[test]
    fn wide_characters_take_two_columns() {
        // "微信" is four columns wide: the cells after it start where the
        // header says, not two characters later.
        let text = "Name        Id                Version  Source\n\
---------------------------------------------\n\
\u{5FAE}\u{4FE1}        Tencent.WeChat    3.9.12   winget\n\
\u{4E2D}\u{6587}\u{8F93}\u{5165}\u{6CD5}    Sogou.\u{2026}           13.0     winget\n";
        let rows = parse_search_table(text);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].name.as_str(), rows[0].id.as_str(), rows[0].version.as_str(), rows[0].source.as_str()), ("\u{5FAE}\u{4FE1}", "Tencent.WeChat", "3.9.12", "winget"));
        assert_eq!((rows[1].id.as_str(), rows[1].version.as_str(), rows[1].truncated), ("Sogou.\u{2026}", "13.0", true));
    }

    #[test]
    fn a_header_in_wide_characters_gives_display_columns() {
        let text = "\u{540D}\u{79F0}        ID                \u{7248}\u{672C}\n\
------------------------------------------\n\
\u{5FAE}\u{4FE1}        Tencent.WeChat    3.9.12\n";
        let rows = parse_winget_table(text);
        assert_eq!((rows[0].name.as_str(), rows[0].id.as_str(), rows[0].version.as_str()), ("\u{5FAE}\u{4FE1}", "Tencent.WeChat", "3.9.12"));
    }

    #[test]
    fn rows_match_apps_by_key_then_by_name_and_version() {
        let apps = [
            InstalledEntry { key: "HKLM32\\AMD_Chipset_IODrivers", name: "AMD Chipset Software", version: "" },
            InstalledEntry { key: "HKCU\\Antigravity", name: "Antigravity 2.5.0", version: "2.5.0" },
            InstalledEntry { key: "MSIX\\Xbox.TCUI_1.24_x64__8", name: "Xbox TCUI", version: "1.24.1" },
            InstalledEntry { key: "HKLM\\Dup1", name: "Dup", version: "1.0" },
            InstalledEntry { key: "HKLM\\Dup2", name: "Dup", version: "2.0" },
        ];
        let rows = vec![
            WingetRow { name: "AMD Chipset Software".into(), id: "ARP\\Machine\\X86\\AMD_Chipset_IODrivers".into(), ..Default::default() },
            WingetRow { name: "Antigravity 2.5.0".into(), id: "Google.Antigravity".into(), version: "2.5.0".into(), ..Default::default() },
            WingetRow { name: "Xbox TCUI".into(), id: "MSIX\\Xbox.TCUI_1.24_x64__8".into(), ..Default::default() },
            WingetRow { name: "Dup".into(), id: "V.Dup".into(), version: "2.0".into(), ..Default::default() },
            WingetRow { name: "Dup".into(), id: "V.Dup".into(), version: "9.9".into(), ..Default::default() },
            WingetRow { name: "Nobody".into(), id: "V.Nobody".into(), ..Default::default() },
        ];
        assert_eq!(match_rows(&apps, &rows), vec![Some(0), Some(1), Some(2), Some(4), Some(3), None]);
    }

    #[test]
    fn output_keeps_the_last_real_lines() {
        let text = "Found Foo [Vendor.Foo] Version 2.0\r\n   - \r   \\ \r\n  \u{2588}\u{2588}\u{2588}\u{2592}\u{2592}  1.00 MB / 2.00 MB\r  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}  2.00 MB / 2.00 MB\r\n\nSuccessfully verified installer hash\nStarting package install...\nSuccessfully installed\n";
        assert_eq!(last_lines(text, 12), "Found Foo [Vendor.Foo] Version 2.0\nSuccessfully verified installer hash\nStarting package install...\nSuccessfully installed");
        assert_eq!(last_lines(text, 2), "Starting package install...\nSuccessfully installed");
        assert_eq!(last_lines("", 5), "");
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_join_with_commas_and_and() {
        assert_eq!(join_names(&[]), "");
        assert_eq!(join_names(&names(&["A"])), "A");
        assert_eq!(join_names(&names(&["A", "B", "C"])), "A, B and C");
    }

    /// A winget HRESULT as the exit code a process returns.
    fn hresult(code: u32) -> i32 {
        code as i32
    }

    #[test]
    fn exit_codes_fall_into_classes() {
        assert_eq!(classify(0), ExitClass::Installed);
        assert_eq!(classify(3010), ExitClass::InstalledRestart);
        assert_eq!(classify(1641), ExitClass::InstalledRestart);
        assert_eq!(classify(hresult(0x8A15_0109)), ExitClass::InstalledRestart);
        assert_eq!(classify(1602), ExitClass::Cancelled);
        assert_eq!(classify(hresult(0x8A15_010C)), ExitClass::Cancelled);
        assert_eq!(classify(hresult(0x8A15_002B)), ExitClass::Skipped("no applicable update found"));
        assert_eq!(classify(hresult(0x8A15_0061)), ExitClass::Skipped("a version of the package is already installed"));
        assert_eq!(classify(hresult(0x8A15_0068)), ExitClass::Skipped("the package has a pin that prevents upgrade"));
        assert_eq!(classify(1618), ExitClass::Failed);
        assert_eq!(classify(hresult(0x8A15_0102)), ExitClass::Failed);
        assert_eq!(classify(hresult(0x8A15_0010)), ExitClass::Failed);
        assert_eq!(classify(1), ExitClass::Failed);
    }

    #[test]
    fn exit_codes_read_as_words_with_the_raw_code() {
        assert_eq!(describe(hresult(0x8A15_0101)), "the app is running; close it and try again (0x8A150101)");
        assert_eq!(describe(hresult(0x8A15_002B)), "no applicable update found (0x8A15002B)");
        // The two labels checked against winget's table.
        assert_eq!(describe(hresult(0x8A15_0061)), "a version of the package is already installed (0x8A150061)");
        assert_eq!(describe(hresult(0x8A15_0010)), "no installer of the package fits this PC (0x8A150010)");
        // A code only the full table knows, words as winget documents them.
        assert_eq!(describe(hresult(0x8A15_0105)), "there's no more space on your PC. Make space, then try again (0x8A150105)");
        assert_eq!(describe(1618), "another installation is in progress (1618)");
        assert_eq!(describe(3010), "restart Windows to finish (3010)");
    }

    #[test]
    fn an_unknown_winget_code_reads_as_a_winget_code_in_hex() {
        assert_eq!(describe(hresult(0x8A15_0FFF)), "winget code 0x8A150FFF");
        assert_eq!(classify(hresult(0x8A15_0FFF)), ExitClass::Failed);
    }

    #[test]
    fn an_unknown_code_outside_winget_reads_as_a_plain_exit_code() {
        assert_eq!(describe(1603), "exit code 1603");
        assert_eq!(describe(1), "exit code 1");
        assert_eq!(describe(-1), "exit code -1");
        assert_eq!(classify(1603), ExitClass::Failed);
    }

    #[test]
    fn an_upgrade_with_a_different_install_technology_is_skipped() {
        assert_eq!(classify(hresult(0x8A15_008E)), ExitClass::Skipped("an upgrade is available but uses a different install technology than the current installation"));
    }

    #[test]
    fn every_documented_code_has_words_once() {
        assert!(CODES.len() > 200);
        assert!(CODES.iter().all(|(_, w)| !w.is_empty()));
        let unique: std::collections::HashSet<u32> = CODES.iter().map(|(c, _)| *c).collect();
        assert_eq!(unique.len(), CODES.len(), "every code once");
    }

    #[test]
    fn descriptions_start_lower_case_unless_with_an_acronym() {
        for (code, w) in CODES {
            let first = w.split(' ').next().unwrap_or_default();
            let acronym = first.len() > 1 && first.chars().all(|c| c.is_ascii_uppercase());
            assert!(acronym || !w.starts_with(|c: char| c.is_uppercase()), "0x{code:08X}: {w}");
        }
    }
}
