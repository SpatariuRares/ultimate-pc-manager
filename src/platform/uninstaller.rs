//! The shared uninstaller: removes an installed program the way Windows
//! lists it, waits for it and every process it starts (until "Stop
//! waiting"), then checks that its entry is gone.
//!
//! - A classic program: the command its uninstall registry entry registered.
//! - A Windows Installer product: `MsiExec.exe /X{code} /passive
//!   /promptrestart`, whatever the entry says. The MSI shows its progress
//!   and asks for administrator rights itself (UAC) when the product is
//!   machine-wide; a closed prompt is a cancelled uninstall (1602).
//! - A Store app (MSIX): its package removed for this account.
//!
//! Every program starts through the [`AddonContext`]'s executor.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{AddonContext, Waited};
use crate::availability::{unavailable_text, Feature};

/// An installed program as Windows lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An uninstall registry entry.
    Registry {
        /// Its view and subkey: "HKLM\\{id}", "HKLM32\\{id}" or "HKCU\\{id}".
        key: String,
        /// The command line it registered (UninstallString; may be empty).
        command: String,
        /// Its Windows Installer product code (empty when it is not one).
        msi_code: String,
    },
    /// A Store package (its full name), removed for this account.
    Msix { package: String },
}

/// Starts uninstallers and waits for them. Cheap to clone; clones share
/// the "Stop waiting" signal.
#[derive(Clone)]
pub struct Uninstaller {
    ctx: AddonContext,
    stop: Arc<AtomicBool>,
}

impl Uninstaller {
    pub fn new(ctx: AddonContext) -> Self {
        Uninstaller { ctx, stop: Default::default() }
    }

    /// Stops waiting for the running program (it keeps running).
    pub fn stop_waiting(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Waits again for the programs started from now on (clears "Stop waiting").
    pub fn wait_again(&self) {
        self.stop.store(false, Ordering::Relaxed);
    }

    /// Uninstalls `name`; `waiting` is told when the wait for its
    /// uninstaller starts (true) and ends (false). Ok once it is gone.
    pub fn uninstall(&self, name: &str, target: &Target, mut waiting: impl FnMut(bool)) -> Result<(), String> {
        // Without the installed-programs registry nothing could tell the entry
        // is gone (or list the program at all): say so for every kind of
        // target, instead of running an uninstaller and reporting it "still
        // installed".
        if let Some(why) = unavailable_text(Feature::UninstallRegistry) {
            return Err(why);
        }
        let (key, command, msi_code) = match target {
            Target::Msix { package } => {
                let v = self.ctx.powershell(REMOVE_SCRIPT, &[("Package", package.as_str())], false)?;
                return if v["ok"].as_bool() == Some(true) {
                    Ok(())
                } else {
                    Err(format!("{name} could not be uninstalled: {}", v["error"].as_str().unwrap_or("unknown error")))
                };
            }
            Target::Registry { key, command, msi_code } => (key, command, msi_code),
        };
        if command.trim().is_empty() && msi_code.is_empty() {
            return Err(not_registered(name));
        }
        // Windows Installer products: the standard removal, whatever the
        // entry says, with a progress window: an MSI run quietly cannot ask
        // for administrator rights.
        let msi = !msi_code.is_empty() && (command.trim().is_empty() || command.to_lowercase().contains("msiexec"));
        let cmd = if msi {
            format!("MsiExec.exe /X{msi_code} /passive /promptrestart")
        } else {
            command.clone()
        };
        if let Some(why) = missing_program(&cmd) {
            return Err(why.replace("%1", name));
        }
        waiting(true);
        let waited = self.start_and_wait(&cmd).map_err(|e| e.replace("%1", name));
        waiting(false);
        let waited = waited?;
        if msi && matches!(waited, Waited::Finished(Some(MSI_CANCELLED))) {
            return Err(cancelled(name));
        }
        if !imp::still_listed(key) {
            return Ok(());
        }
        Err(match waited {
            Waited::Finished(_) => format!("{name} is still installed after its uninstaller closed."),
            Waited::Stopped => format!("Stopped waiting for the uninstaller of {name}."),
            Waited::TimedOut => format!("The uninstaller for {name} was still running after an hour."),
        })
    }

    /// Starts a command line and waits for it and every process it starts,
    /// until "Stop waiting". Errors may hold "%1" for the program's name.
    pub fn start_and_wait(&self, cmd: &str) -> Result<Waited, String> {
        if let Some(why) = missing_program(cmd) {
            return Err(why);
        }
        let (file, params) = split_command(cmd);
        self.ctx.run_and_wait(&file, &params, &self.stop)
    }
}

/// Why `name` can't be uninstalled when it registered no uninstaller.
pub fn not_registered(name: &str) -> String {
    format!("{name} did not register an uninstaller with Windows.")
}

/// Windows Installer's exit code for a cancelled uninstall (also a closed
/// administrator prompt).
const MSI_CANCELLED: i32 = 1602;

/// Why `name` is still installed when its uninstaller was cancelled
/// (Windows Installer's 1602: also a closed administrator prompt).
pub fn cancelled(name: &str) -> String {
    format!("{name} was not uninstalled: the uninstall was cancelled.")
}

/// Removes a Store package for this account.
const REMOVE_SCRIPT: &str = r#"param([string]$Out, [string]$Package)
$res = [ordered]@{ ok = $false; error = '' }
try {
  Remove-AppxPackage -Package $Package -ErrorAction Stop
  $res.ok = $true
} catch { $res.error = $_.Exception.Message }
ConvertTo-Json -InputObject $res | Out-File -FilePath $Out -Encoding utf8
"#;

/// Why a command line can't start because the program it names by path is
/// gone. May hold "%1" for the name of the app that registered it.
pub fn missing_program(cmd: &str) -> Option<String> {
    let (file, _) = split_command(cmd);
    (file.contains('\\') && !std::path::Path::new(&file).exists()).then(|| format!("The uninstaller %1 registered is missing ({file})."))
}

/// Splits a command line into program and arguments; copes with unquoted
/// paths that contain spaces ("C:\Program Files\X\uninst.exe /S").
pub fn split_command(cmd: &str) -> (String, String) {
    let s = cmd.trim();
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (expand_env(&rest[..end]), rest[end + 1..].trim().to_string());
        }
        return (expand_env(rest), String::new());
    }
    let lower = s.to_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(".exe") {
        let end = from + i + 4;
        if end == s.len() || s[end..].starts_with(' ') {
            return (expand_env(&s[..end]), s[end..].trim().to_string());
        }
        from = end;
    }
    match s.split_once(' ') {
        Some((a, b)) => (expand_env(a), b.trim().to_string()),
        None => (expand_env(s), String::new()),
    }
}

/// Expands %VAR% the way the registry's REG_EXPAND_SZ values expect.
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        out.push_str(&rest[..a]);
        let after = &rest[a + 1..];
        match after.find('%') {
            Some(b) => {
                let name = &after[..b];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => out.push_str(&rest[a..a + b + 2]),
                }
                rest = &after[b + 1..];
            }
            None => {
                out.push_str(&rest[a..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The uninstall registry entry `key` ("HKLM\{id}", "HKLM32\{id}" or
/// "HKCU\{id}") as a target: the command it registered and, for a Windows
/// Installer product, its product code. `None` when the entry is not there.
pub fn registry_entry(key: &str) -> Option<Target> {
    imp::entry(key).map(|(command, msi)| {
        let sub = key.split_once('\\').map(|(_, s)| s).unwrap_or_default();
        let msi_code = if msi && sub.starts_with('{') { sub.to_string() } else { String::new() };
        Target::Registry { key: key.to_string(), command, msi_code }
    })
}

// -------------------------------------------------------------- platform

#[cfg(not(windows))]
mod imp {
    /// The uninstall registry is Windows only. [`Uninstaller::uninstall`]
    /// reports that before it gets here, so "still listed" is never asked.
    pub fn still_listed(_: &str) -> bool {
        true
    }

    /// The uninstall registry is Windows only: no entry is there.
    pub fn entry(_: &str) -> Option<(String, bool)> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use winreg::enums::*;
    use winreg::RegKey;

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    const UNINSTALL32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

    /// Whether the uninstall registry still has the entry `key`
    /// ("HKLM\{id}", "HKLM32\{id}" or "HKCU\{id}").
    pub fn still_listed(key: &str) -> bool {
        open(key).is_some()
    }

    /// The entry `key`'s UninstallString and whether it is a Windows
    /// Installer product (WindowsInstaller = 1).
    pub fn entry(key: &str) -> Option<(String, bool)> {
        let k = open(key)?;
        let command = k.get_value::<String, _>("UninstallString").unwrap_or_default().trim().to_string();
        Some((command, k.get_value::<u32, _>("WindowsInstaller").unwrap_or(0) == 1))
    }

    /// Opens the entry `key`, if it is there.
    fn open(key: &str) -> Option<RegKey> {
        let (hive, sub) = key.split_once('\\')?;
        let (root, path) = match hive {
            "HKLM" => (RegKey::predef(HKEY_LOCAL_MACHINE), UNINSTALL),
            "HKLM32" => (RegKey::predef(HKEY_LOCAL_MACHINE), UNINSTALL32),
            "HKCU" => (RegKey::predef(HKEY_CURRENT_USER), UNINSTALL),
            _ => return None,
        };
        root.open_subkey(format!(r"{path}\{sub}")).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::availability::unavailable_in_test;
    use crate::platform::Launch;

    /// The uninstaller of the windowless add-on host, and its test executor.
    fn through_the_host() -> (Uninstaller, Arc<crate::platform::RecordingRunner>) {
        let (host, runner) = crate::addons::AddonHost::for_tests(&serde_json::json!({}));
        (Uninstaller::new(host.context().clone()), runner)
    }

    fn registry(command: &str, msi_code: &str) -> Target {
        Target::Registry { key: "HKCU\\UPCM-110-NotReallyInstalled".into(), command: command.into(), msi_code: msi_code.into() }
    }

    fn program(file: &str, params: &str) -> Launch {
        Launch::Program { file: file.into(), params: params.into() }
    }

    #[test]
    fn an_unavailable_uninstall_registry_is_reported_and_nothing_is_started() {
        let _unavailable = unavailable_in_test(&[Feature::UninstallRegistry]);
        let (u, runner) = through_the_host();
        let store = Target::Msix { package: "Calc_1.0_x64__abc".into() };
        for target in [registry("uninst.exe /S", ""), store] {
            let mut told = Vec::new();
            let result = u.uninstall("Tool", &target, |w| told.push(w));
            assert_eq!(result, Err("Not available on this system.".into()), "{target:?}");
            assert!(runner.launches().is_empty() && told.is_empty(), "no uninstaller runs and nothing waits for it: {target:?}");
        }
    }

    #[test]
    fn a_registry_entry_runs_the_command_it_registered() {
        let (u, runner) = through_the_host();
        let mut told = Vec::new();
        let result = u.uninstall("Tool", &registry("uninst.exe /S", ""), |w| told.push(w));
        assert_eq!(runner.launches(), vec![program("uninst.exe", "/S")]);
        assert_eq!(result, Err("Not launched: test executor.".into()), "the executor's failure is the outcome");
        assert_eq!(told, vec![true, false], "the wait starts and ends");
    }

    #[test]
    fn a_windows_installer_product_is_removed_with_msiexec_x() {
        let (u, runner) = through_the_host();
        let _ = u.uninstall("Msi", &registry("MsiExec.exe /I{ABC}", "{ABC}"), |_| {});
        let _ = u.uninstall("Msi", &registry("", "{DEF}"), |_| {});
        let _ = u.uninstall("Own", &registry("C:\\Windows\\notepad.exe /remove", "{GHI}"), |_| {});
        assert_eq!(
            runner.launches(),
            vec![
                program("MsiExec.exe", "/X{ABC} /passive /promptrestart"),
                program("MsiExec.exe", "/X{DEF} /passive /promptrestart"),
                program("C:\\Windows\\notepad.exe", "/remove")
            ],
            "MsiExec /X whatever the entry says, with a progress window so it can ask for administrator rights; an uninstaller of its own runs as registered"
        );
    }

    #[test]
    fn a_windows_installer_product_cancelled_or_whose_prompt_was_closed_is_not_uninstalled() {
        let (u, runner) = through_the_host();
        runner.prepare(crate::platform::When::Arg("/X{ABC}".into()), 1602, "");
        assert_eq!(u.uninstall("Msi", &registry("MsiExec.exe /I{ABC}", "{ABC}"), |_| {}), Err(cancelled("Msi")));
        runner.prepare(crate::platform::When::Arg("/S".into()), 1602, "");
        assert_eq!(u.uninstall("Tool", &registry("uninst.exe /S", ""), |_| {}), Ok(()), "another uninstaller's 1602 means nothing: its entry is gone");
    }

    #[test]
    fn a_store_app_is_removed_for_this_account() {
        let (u, runner) = through_the_host();
        let mut told = Vec::new();
        let result = u.uninstall("Calculator", &Target::Msix { package: "Calc_1.0_x64__abc".into() }, |w| told.push(w));
        assert_eq!(
            runner.launches(),
            vec![Launch::PowerShell { script: REMOVE_SCRIPT.into(), args: vec![("Package".into(), "Calc_1.0_x64__abc".into())], elevated: false }]
        );
        assert!(result.is_err());
        assert!(told.is_empty(), "nothing to wait for");
    }

    #[test]
    fn stop_waiting_ends_the_wait_and_the_uninstaller_keeps_running() {
        let (u, runner) = through_the_host();
        runner.hold();
        let told = Arc::new(std::sync::Mutex::new(Vec::new()));
        let job = {
            let (u, told) = (u.clone(), told.clone());
            std::thread::spawn(move || u.uninstall("Tool", &registry("uninst.exe /S", ""), |w| told.lock().unwrap().push(w)))
        };
        runner.wait_for(1);
        assert_eq!(*told.lock().unwrap(), vec![true], "waiting for the uninstaller");
        u.stop_waiting();
        let result = job.join().unwrap();
        runner.release();
        assert_eq!(runner.stopped(), 1, "the wait ended early");
        assert_eq!(result, Ok(()), "its entry is gone");
        assert_eq!(*told.lock().unwrap(), vec![true, false]);

        // The next uninstall waits again.
        u.wait_again();
        let _ = u.uninstall("Tool", &registry("uninst.exe /S", ""), |_| {});
        assert_eq!(runner.stopped(), 1);
    }

    #[test]
    fn a_registered_uninstaller_that_is_gone_starts_nothing() {
        let (u, runner) = through_the_host();
        let result = u.uninstall("Tool", &registry("\"C:\\Definitely\\Not\\Here\\uninst.exe\" /S", ""), |_| {});
        assert_eq!(result, Err("The uninstaller Tool registered is missing (C:\\Definitely\\Not\\Here\\uninst.exe).".into()));
        assert!(runner.launches().is_empty());
    }

    #[test]
    fn a_registry_entry_that_is_not_there_is_no_target() {
        assert_eq!(registry_entry("HKCU\\UPCM-118-NotReallyInstalled"), None);
        assert_eq!(registry_entry("nowhere"), None);
    }

    #[test]
    fn an_entry_without_an_uninstaller_starts_nothing() {
        let (u, runner) = through_the_host();
        assert_eq!(u.uninstall("Orphan", &registry("  ", ""), |_| {}), Err(not_registered("Orphan")));
        assert!(runner.launches().is_empty());
    }
}
