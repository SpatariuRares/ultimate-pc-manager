//! Whether winget can be used on this PC, and how to repair it when not.
//! The check asks `winget --version`; when winget cannot even be started,
//! it asks PowerShell whether App Installer (the package that brings
//! winget) is there for this user, to tell "not registered" from "missing".

use std::sync::atomic::AtomicBool;

use crate::platform::{AddonContext, Ran};

/// The oldest winget the add-on works with: the newest of the arguments it
/// passes need it. `--no-upgrade` (install) and `--disable-interactivity`
/// (every command) are absent from winget-cli v1.3.2691 and present in
/// v1.4.10173 (src/AppInstallerCLICore/Commands/InstallCommand.cpp and
/// Argument.cpp `GetCommon`); `--include-versions` (export), `--scope`
/// (install), `--version`, `--log`, `--silent`, `--exact`, `--source` and
/// the agreement flags are already in v1.3.2691.
pub const MIN_VERSION: [u32; 3] = [1, 4, 0];

/// Where to get App Installer (winget) when it is missing, e.g. on Windows
/// editions without the Microsoft Store such as LTSC.
pub const GET_WINGET: &str = "https://aka.ms/getwinget";

/// App Installer's page in the Microsoft Store, to update it.
pub const STORE_PAGE: &str = "ms-windows-store://pdp/?ProductId=9NBLGGH4NNS1";

/// The PowerShell command that prints App Installer's version for this
/// user (nothing when it is not there).
pub const FIND_APP_INSTALLER: &str = "(Get-AppxPackage -Name Microsoft.DesktopAppInstaller).Version";

/// The PowerShell command that registers App Installer again for this
/// user (no administrator rights needed).
pub const REGISTER_APP_INSTALLER: &str = "Add-AppxPackage -RegisterByFamilyName -MainPackage Microsoft.DesktopAppInstaller_8wekyb3d8bbwe";

/// Whether winget is ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Neither winget nor App Installer is on this PC.
    Missing,
    /// App Installer is there but winget does not start or does not work.
    NotRegistered,
    /// winget works but is older than [`MIN_VERSION`]; its version.
    TooOld(String),
    /// winget works; its version.
    Ready(String),
}

/// What repairs a readiness that is not [`Readiness::Ready`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Repair {
    /// Opens this link (a web page or the Store).
    Open(&'static str),
    /// Registers App Installer again through PowerShell.
    Register,
}

impl Readiness {
    /// The headline the page shows.
    pub fn headline(&self) -> String {
        match self {
            Readiness::Missing => "winget is not installed".into(),
            Readiness::NotRegistered => "winget is not registered".into(),
            Readiness::TooOld(v) => format!("winget {v} is too old"),
            Readiness::Ready(v) => format!("winget {v} is ready"),
        }
    }

    /// What it means and how to fix it, in words.
    pub fn detail(&self) -> String {
        let min = version_text(MIN_VERSION);
        match self {
            Readiness::Missing => "Package Manager needs winget, which comes with App Installer. Windows editions without the \
                Microsoft Store (such as LTSC) do not include it: get it from Microsoft's page, then check again."
                .into(),
            Readiness::NotRegistered => "App Installer is on this PC but winget does not start for your account. Registering \
                App Installer again usually fixes it; no administrator rights are needed."
                .into(),
            Readiness::TooOld(_) => format!("Package Manager needs winget {min} or later. Update App Installer from the Microsoft Store, then check again."),
            Readiness::Ready(_) => "You can search and install programs from the winget catalogs.".into(),
        }
    }

    /// The repair offered, if any, with its button's text.
    pub fn repair(&self) -> Option<(Repair, &'static str)> {
        match self {
            Readiness::Missing => Some((Repair::Open(GET_WINGET), "Get App Installer")),
            Readiness::NotRegistered => Some((Repair::Register, "Register App Installer")),
            Readiness::TooOld(_) => Some((Repair::Open(STORE_PAGE), "Update in the Microsoft Store")),
            Readiness::Ready(_) => None,
        }
    }
}

/// "1.4.0".
fn version_text(v: [u32; 3]) -> String {
    format!("{}.{}.{}", v[0], v[1], v[2])
}

/// The version `winget --version` printed ("v1.9.25200" gives "1.9.25200"
/// and [1, 9, 25200]); None when no line holds one.
pub fn parse_version(output: &str) -> Option<(String, [u32; 3])> {
    output.lines().find_map(|line| {
        let text = line.trim().strip_prefix('v')?;
        let core = text.split(['-', '+', ' ']).next()?;
        let mut parts = core.split('.').map(|p| p.parse::<u32>().ok());
        let v = [parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(0)];
        Some((text.to_string(), v))
    })
}

/// The readiness the two answers give: winget's to `--version`, and (only
/// asked when winget did not start) PowerShell's to [`FIND_APP_INSTALLER`].
pub fn judge(winget: &Answer, app_installer: Option<&Answer>) -> Readiness {
    match winget {
        Answer::Exited(0, out) => match parse_version(out) {
            Some((text, v)) if v < MIN_VERSION => Readiness::TooOld(text),
            Some((text, _)) => Readiness::Ready(text),
            None => Readiness::NotRegistered,
        },
        Answer::Exited(..) => Readiness::NotRegistered,
        Answer::NotStarted => match app_installer {
            Some(Answer::Exited(0, out)) if !out.trim().is_empty() => Readiness::NotRegistered,
            _ => Readiness::Missing,
        },
    }
}

/// How a program asked during the check answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// It could not be started.
    NotStarted,
    /// It ended with this code and output.
    Exited(i32, String),
}

/// Runs `program` with `args` through the context's generic run, its
/// output in a temporary file (deleted afterwards). None when `stop` ended it.
pub fn ask(context: &AddonContext, program: &str, args: &[&str], stop: &AtomicBool) -> Option<Answer> {
    let output = temp_output();
    let ran = context.run(program, args, &output, stop, &mut |_| {});
    let text = std::fs::read(&output).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let _ = std::fs::remove_file(&output);
    match ran {
        Ran::Exited(code) => Some(Answer::Exited(code, text)),
        Ran::NotStarted(_) => Some(Answer::NotStarted),
        Ran::Stopped => None,
    }
}

/// A temporary file name no other run shares (process id, time, counter).
pub fn temp_output() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("upcm-package-manager-{}-{nanos}-{n}.txt", std::process::id()))
}

/// PowerShell's arguments to run `command` without profile or prompts.
pub fn powershell_args(command: &str) -> [&str; 4] {
    ["-NoProfile", "-NonInteractive", "-Command", command]
}

/// Checks winget (None when `stop` ended the check).
pub fn check(context: &AddonContext, stop: &AtomicBool) -> Option<Readiness> {
    let winget = ask(context, "winget", &["--version"], stop)?;
    let app_installer = match winget {
        Answer::NotStarted => Some(ask(context, "powershell.exe", &powershell_args(FIND_APP_INSTALLER), stop)?),
        Answer::Exited(..) => None,
    };
    Some(judge(&winget, app_installer.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_read_from_wingets_answer() {
        assert_eq!(parse_version("v1.9.25200\r\n"), Some(("1.9.25200".into(), [1, 9, 25200])));
        assert_eq!(parse_version("v1.10.320-preview"), Some(("1.10.320-preview".into(), [1, 10, 320])));
        assert_eq!(parse_version("Windows Package Manager\nv1.6.3133"), Some(("1.6.3133".into(), [1, 6, 3133])));
        assert_eq!(parse_version("An unexpected error occurred"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn the_four_states_come_from_the_answers() {
        let exited = |code, out: &str| Answer::Exited(code, out.into());
        assert_eq!(judge(&exited(0, "v1.9.25200"), None), Readiness::Ready("1.9.25200".into()));
        assert_eq!(judge(&exited(0, "v1.4.0"), None), Readiness::Ready("1.4.0".into()), "the minimum itself is enough");
        assert_eq!(judge(&exited(0, "v1.4.10173"), None), Readiness::Ready("1.4.10173".into()), "the first 1.4 release has every argument it passes");
        assert_eq!(judge(&exited(0, "v1.3.2691"), None), Readiness::TooOld("1.3.2691".into()), "1.3 has no --no-upgrade nor --disable-interactivity");
        assert_eq!(judge(&exited(-1978335226, "error"), None), Readiness::NotRegistered, "it starts but fails");
        assert_eq!(judge(&Answer::NotStarted, Some(&exited(0, "1.21.3482.0\r\n"))), Readiness::NotRegistered);
        assert_eq!(judge(&Answer::NotStarted, Some(&exited(0, "\r\n"))), Readiness::Missing);
        assert_eq!(judge(&Answer::NotStarted, Some(&Answer::NotStarted)), Readiness::Missing);
    }

    #[test]
    fn each_state_but_ready_offers_its_repair() {
        assert_eq!(Readiness::Missing.repair().map(|r| r.0), Some(Repair::Open(GET_WINGET)));
        assert_eq!(Readiness::NotRegistered.repair().map(|r| r.0), Some(Repair::Register));
        assert_eq!(Readiness::TooOld("1.2.0".into()).repair().map(|r| r.0), Some(Repair::Open(STORE_PAGE)));
        assert_eq!(Readiness::Ready("1.9.0".into()).repair(), None);
        assert!(Readiness::Missing.detail().contains("LTSC"));
        assert!(Readiness::TooOld("1.2.0".into()).detail().contains("1.4.0"), "says the minimum version");
        assert!(Readiness::TooOld("1.2.0".into()).headline().contains("1.2.0"), "says the version found");
    }
}
