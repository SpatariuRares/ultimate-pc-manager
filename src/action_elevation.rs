//! Starting ONE program with administrator rights, the way each system asks
//! for them. The app itself never runs elevated for this.
//!
//! | System  | How the one action is elevated                                   |
//! |---------|------------------------------------------------------------------|
//! | Windows | not offered here: UAC is asked through the elevated PowerShell   |
//! |         | path of `platform` (unchanged)                                   |
//! | Linux   | `pkexec` (polkit), the program and its arguments as separate     |
//! |         | argv entries                                                     |
//! | macOS   | the administrator authorisation prompt, through `osascript`      |
//!
//! # How arguments are passed (injection safety)
//!
//! * The text the user typed is split into words by [`split_words`] (quotes
//!   and backslashes only; no variable, glob or command substitution).
//! * Linux: every word is one argv entry of the `pkexec` process
//!   (`pkexec [env NAME=value ...] program arg ...`). No shell string is
//!   ever built. A program name that starts with `-` or contains `=` is
//!   refused, because `pkexec` or `env` would read it as an option or an
//!   assignment instead of a program.
//! * macOS: AppleScript cannot take an argv without a script, so the script
//!   is fixed text (`on run argv` / `do shell script (item 1 of argv) with
//!   administrator privileges` / `end run`) and the command is passed as the
//!   one argument of `osascript`, never spliced into the script. That one
//!   argument is the only shell string: every word is wrapped in single
//!   quotes with each embedded `'` written as `'\''`, so the shell sees
//!   exactly the words typed.
//!
//! The pure parts here take the system and the environment as values and
//! the launch as a closure, so the tests cover every system on any host.

use crate::app_folders::System;

/// How an elevated launch ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spawned {
    /// The launcher itself could not be found (no `pkexec`, no `osascript`).
    NotFound,
    /// The launcher could not be started for another reason.
    Failed(String),
    /// The launcher ended with this exit code and this error output.
    Exited { code: i32, stderr: String },
}

/// What the Linux dialog says when `pkexec` is not installed.
pub const PKEXEC_MISSING: &str = "Running as administrator needs pkexec, which is not installed. Install polkit (the \"policykit-1\" or \"polkit\" package) and try again.";
/// What it says when the authentication prompt was dismissed.
pub const PERMISSION_DISMISSED: &str = "Administrator permission was not granted: the prompt was dismissed.";
/// What it says when permission was refused or could not be asked for.
pub const PERMISSION_REFUSED: &str = "Administrator permission was refused. You are not allowed to run programs as administrator, the password was wrong, no authentication agent is running, or the program was not found.";

/// Environment variables a graphical program needs to show its window
/// again once `pkexec` has cleaned the environment.
const DISPLAY_VARIABLES: [&str; 3] = ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY"];

/// Splits what the user typed into words: spaces separate, `'...'` and
/// `"..."` group (spaces included), a backslash outside single quotes
/// escapes the next character. Nothing else is interpreted.
pub fn split_words(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("A quote is not closed.".into()),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c) => word.push(c),
                            None => return Err("A quote is not closed.".into()),
                        },
                        Some(c) => word.push(c),
                        None => return Err("A quote is not closed.".into()),
                    }
                }
            }
            '\\' => {
                started = true;
                match chars.next() {
                    Some(c) => word.push(c),
                    None => word.push('\\'),
                }
            }
            c => {
                started = true;
                word.push(c);
            }
        }
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

/// Wraps `word` in single quotes for a POSIX shell.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The launcher and its arguments that start `cmdline` elevated on
/// `system`. `env` answers environment variables.
pub fn elevated_argv(system: System, cmdline: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Vec<String>, String> {
    let words = split_words(cmdline)?;
    let Some(program) = words.first() else {
        return Err("Type the name of a program, folder or document.".into());
    };
    match system {
        System::Windows => Err("Windows asks for administrator permission (UAC) when an action needs it.".into()),
        System::Linux => {
            if program.starts_with('-') || program.contains('=') {
                return Err(format!("\"{program}\" can't be started as administrator: type the name or path of a program."));
            }
            let mut argv = vec!["pkexec".to_string()];
            let shown: Vec<String> = DISPLAY_VARIABLES.iter().filter_map(|name| env(name).filter(|v| !v.is_empty()).map(|v| format!("{name}={v}"))).collect();
            if !shown.is_empty() {
                argv.push("env".into());
                argv.extend(shown);
            }
            argv.extend(words);
            Ok(argv)
        }
        System::MacOs => {
            let command = format!("{} >/dev/null 2>&1 &", words.iter().map(|w| shell_quote(w)).collect::<Vec<_>>().join(" "));
            Ok(["osascript", "-e", "on run argv", "-e", "do shell script (item 1 of argv) with administrator privileges", "-e", "end run"]
                .iter()
                .map(|s| s.to_string())
                .chain([command])
                .collect())
        }
    }
}

/// The text to say for how the launcher ended (`Ok` when the program
/// started or ran to its end without the system refusing).
pub fn interpret(system: System, spawned: Spawned) -> Result<(), String> {
    match (system, spawned) {
        (System::Linux, Spawned::NotFound) => Err(PKEXEC_MISSING.into()),
        (System::MacOs, Spawned::NotFound) => Err("Running as administrator needs osascript, which was not found on this system.".into()),
        (_, Spawned::NotFound) => Err("The program that asks for administrator permission was not found.".into()),
        (_, Spawned::Failed(e)) => Err(format!("Could not ask for administrator permission: {e}")),
        // pkexec: 126 = the prompt was dismissed, 127 = not authorised, the
        // authentication failed or pkexec could not run the program. Any
        // other code is the program's own exit code.
        (System::Linux, Spawned::Exited { code: 126, .. }) => Err(PERMISSION_DISMISSED.into()),
        (System::Linux, Spawned::Exited { code: 127, .. }) => Err(PERMISSION_REFUSED.into()),
        (System::MacOs, Spawned::Exited { code, stderr }) if code != 0 => {
            if stderr.contains("-128") {
                Err(PERMISSION_DISMISSED.into())
            } else {
                Err(format!("Administrator permission was refused or the program could not start. {}", stderr.trim()).trim().to_string())
            }
        }
        _ => Ok(()),
    }
}

/// Starts `cmdline` elevated on `system`: builds the launch, hands its argv
/// to `launch` (which waits for the launcher to end) and words the result.
pub fn run(system: System, cmdline: &str, env: &dyn Fn(&str) -> Option<String>, launch: &dyn Fn(&[String]) -> Spawned) -> Result<(), String> {
    let argv = elevated_argv(system, cmdline.trim(), env)?;
    interpret(system, launch(&argv))
}

/// The real launch: runs `argv` and waits for it to end.
pub fn spawn_and_wait(argv: &[String]) -> Spawned {
    let Some((program, args)) = argv.split_first() else {
        return Spawned::Failed("nothing to run".into());
    };
    match std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).output() {
        Ok(out) => Spawned::Exited { code: out.status.code().unwrap_or(-1), stderr: String::from_utf8_lossy(&out.stderr).into_owned() },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Spawned::NotFound,
        Err(e) => Spawned::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn words_split_at_spaces_and_group_in_quotes() {
        assert_eq!(split_words(r#"gedit "my file.txt" 'a b' c\ d"#).unwrap(), ["gedit", "my file.txt", "a b", "c d"]);
        assert_eq!(split_words("  ").unwrap(), Vec::<String>::new());
        assert!(split_words("gedit 'open").is_err());
    }

    #[test]
    fn linux_passes_each_word_as_its_own_argument_to_pkexec() {
        let argv = elevated_argv(System::Linux, "gedit \"/etc/my file\"; rm -rf ~ $(id)", &no_env).unwrap();
        assert_eq!(argv, ["pkexec", "gedit", "/etc/my file;", "rm", "-rf", "~", "$(id)"], "no shell: metacharacters stay plain text");
    }

    #[test]
    fn linux_keeps_the_window_of_a_graphical_program() {
        let env = |n: &str| (n == "DISPLAY").then(|| ":0".to_string());
        assert_eq!(elevated_argv(System::Linux, "gparted", &env).unwrap(), ["pkexec", "env", "DISPLAY=:0", "gparted"]);
    }

    #[test]
    fn linux_refuses_a_program_that_pkexec_or_env_would_read_as_an_option_or_assignment() {
        for bad in ["-i foo", "--version", "LD_PRELOAD=x gedit"] {
            assert!(elevated_argv(System::Linux, bad, &no_env).is_err(), "{bad}");
        }
    }

    #[test]
    fn macos_passes_a_fixed_script_and_one_quoted_command() {
        let argv = elevated_argv(System::MacOs, r#"open "it's here" $(id)"#, &no_env).unwrap();
        assert_eq!(&argv[..7], ["osascript", "-e", "on run argv", "-e", "do shell script (item 1 of argv) with administrator privileges", "-e", "end run"]);
        assert_eq!(argv.len(), 8);
        assert_eq!(argv[7], r#"'open' 'it'\''s here' '$(id)' >/dev/null 2>&1 &"#);
    }

    #[test]
    fn an_empty_command_is_refused_on_every_system() {
        for s in [System::Linux, System::MacOs] {
            assert_eq!(elevated_argv(s, "   ", &no_env), Err("Type the name of a program, folder or document.".into()));
        }
    }

    #[test]
    fn pkexec_exit_codes_are_worded() {
        let exit = |code| Spawned::Exited { code, stderr: String::new() };
        assert_eq!(interpret(System::Linux, exit(0)), Ok(()));
        assert_eq!(interpret(System::Linux, exit(3)), Ok(()), "the program's own exit code is not a refusal");
        assert_eq!(interpret(System::Linux, exit(126)), Err(PERMISSION_DISMISSED.into()));
        assert_eq!(interpret(System::Linux, exit(127)), Err(PERMISSION_REFUSED.into()));
        assert_eq!(interpret(System::Linux, Spawned::NotFound), Err(PKEXEC_MISSING.into()));
    }

    #[test]
    fn macos_cancel_and_failure_are_worded() {
        let exit = |code, e: &str| Spawned::Exited { code, stderr: e.into() };
        assert_eq!(interpret(System::MacOs, exit(0, "")), Ok(()));
        assert_eq!(interpret(System::MacOs, exit(1, "execution error: User canceled. (-128)")), Err(PERMISSION_DISMISSED.into()));
        assert!(interpret(System::MacOs, exit(1, "bad")).unwrap_err().contains("refused"));
    }
}
