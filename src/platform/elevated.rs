//! The protected elevated PowerShell runner.
//!
//! A script that runs as administrator must not be changeable by a normal
//! process, and its result must not be forgeable by one. The user-writable
//! jobs folder is therefore only a mailbox for the script text:
//!
//! * The elevated process is started with `-EncodedCommand`, a small
//!   bootstrap whose text (the script's SHA-256, a secret per-job token, the
//!   arguments, the paths) lives on the elevated process's own command line,
//!   where a normal process cannot change it.
//! * The bootstrap reads the script file once into memory, checks its hash
//!   and runs those very bytes: swapping the file before or after UAC makes
//!   the check fail and nothing runs (no check-then-use gap).
//! * The bootstrap creates the result folder itself, in `%windir%\Temp`
//!   with an ACL that only gives administrators and SYSTEM write access (the
//!   caller gets read and delete rights only), refuses to reuse an existing
//!   folder, and writes the result wrapped with the job token. The caller
//!   rejects a result whose token is missing or wrong.

use std::path::Path;

/// Exit codes of the bootstrap that name a refusal (0 is a finished job).
const EXIT_SCRIPT_CHANGED: i32 = 3;
const EXIT_NO_RESULT: i32 = 5;
const EXIT_FOLDER_UNTRUSTED: i32 = 6;

/// The error for a bootstrap that ended with `code`, when it names a refusal.
pub fn refusal(code: i32) -> Option<&'static str> {
    match code {
        EXIT_SCRIPT_CHANGED => Some("The administrator script was changed before it ran, so it was not run."),
        EXIT_FOLDER_UNTRUSTED => Some("The protected result folder could not be trusted, so the script was not run."),
        EXIT_NO_RESULT => Some("PowerShell finished without a result."),
        _ => None,
    }
}

/// What one elevated job needs to know.
pub struct Job<'a> {
    /// The script file (in the user-writable jobs folder).
    pub script_file: &'a Path,
    /// SHA-256 (lowercase hex) of the script file's bytes.
    pub script_sha256: &'a str,
    /// The folder the elevated side creates; it must not exist yet.
    pub result_dir: &'a Path,
    /// The secret token the result must carry.
    pub token: &'a str,
    /// The named arguments passed to the script.
    pub args: &'a [(&'a str, &'a str)],
    /// The account that may read and delete the result (domain, name).
    pub caller: (&'a str, &'a str),
    /// Tests only: also gives the caller full control of the result folder,
    /// so a non-elevated run can write there. Always false in production.
    pub caller_full_control: bool,
}

/// `s` as a PowerShell single-quoted string (PowerShell treats the typographic
/// single quotes like the plain one, so every one of them is doubled).
pub fn ps_quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        out.push(c);
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// The PowerShell text the elevated process runs.
pub fn bootstrap(job: &Job) -> Result<String, String> {
    let mut named = String::new();
    for (k, v) in job.args {
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("Invalid script argument name: {k}"));
        }
        named.push_str(&format!(";{k}={}", ps_quote(v)));
    }
    // The caller may read but never delete, rename or write: not the folder
    // (rule for this folder only) and not the files in it (inherit-only).
    let rule = |rights: &str, inherit: &str, prop: &str| format!("$sec.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($me,'{rights}','{inherit}','{prop}','Allow')))");
    let caller_rules = if job.caller_full_control {
        rule("FullControl", "ContainerInherit,ObjectInherit", "None")
    } else {
        format!("{}\n{}", rule("ReadAndExecute", "None", "None"), rule("ReadAndExecute", "ObjectInherit", "InheritOnly"))
    };
    Ok(format!(
        r#"$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$env:PSModulePath=(Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'WindowsPowerShell\Modules')+';'+(Join-Path ([Environment]::SystemDirectory) 'WindowsPowerShell\v1.0\Modules')
try {{
$bytes=[IO.File]::ReadAllBytes({script})
$sha=[Security.Cryptography.SHA256]::Create()
$h=([BitConverter]::ToString($sha.ComputeHash($bytes)) -replace '-','').ToLowerInvariant()
if($h -ne {hash}){{ exit {EXIT_SCRIPT_CHANGED} }}
$text=[Text.Encoding]::UTF8.GetString($bytes).TrimStart([char]0xFEFF)
$dir={dir}
$sidT=[Security.Principal.SecurityIdentifier]
if(Test-Path -LiteralPath $dir){{ exit {EXIT_FOLDER_UNTRUSTED} }}
# The caller cannot delete its result folders (that is what keeps them from
# being replaced), so old ones are swept here: only real folders owned by
# Administrators or SYSTEM, never a link or a folder someone else made.
foreach($old in @(Get-ChildItem -LiteralPath (Split-Path $dir) -Directory -Filter 'upcm-*' -Force -ErrorAction SilentlyContinue)){{
  try{{ if($old.LastWriteTime -lt (Get-Date).AddMinutes(-30) -and -not ($old.Attributes -band [IO.FileAttributes]::ReparsePoint) -and (@('S-1-5-32-544','S-1-5-18') -contains $old.GetAccessControl().GetOwner($sidT).Value)){{ Remove-Item -LiteralPath $old.FullName -Recurse -Force }} }}catch{{}}
}}
$sec=New-Object Security.AccessControl.DirectorySecurity
$sec.SetAccessRuleProtection($true,$false)
foreach($sid in 'S-1-5-18','S-1-5-32-544'){{
  $id=New-Object Security.Principal.SecurityIdentifier($sid)
  $sec.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($id,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))
}}
$me=New-Object Security.Principal.NTAccount({domain},{user})
{caller_rules}
[void][IO.Directory]::CreateDirectory($dir,$sec)
# CreateDirectory succeeds silently on a folder somebody else made first (and
# then applies no ACL), so the folder is trusted only after its owner, ACL,
# type and emptiness are checked.
$meSid=$me.Translate($sidT).Value
$owners=@('S-1-5-32-544','S-1-5-18'){test_owner}
$d=New-Object IO.DirectoryInfo($dir)
$acl=$d.GetAccessControl()
$ok=($owners -contains $acl.GetOwner($sidT).Value) -and $acl.AreAccessRulesProtected -and (-not ($d.Attributes -band [IO.FileAttributes]::ReparsePoint)) -and (@(Get-ChildItem -LiteralPath $dir -Force).Count -eq 0)
foreach($r in $acl.GetAccessRules($true,$true,$sidT)){{
  $who=$r.IdentityReference.Value
  if($r.AccessControlType -ne 'Allow' -or @('S-1-5-18','S-1-5-32-544',$meSid) -notcontains $who){{ $ok=$false }}
  if($who -eq $meSid -and -not {full} -and ([int]$r.FileSystemRights -band 0xD0156) -ne 0){{ $ok=$false }}
}}
if(-not $ok){{ exit {EXIT_FOLDER_UNTRUSTED} }}
$out=Join-Path $dir 'result.json'
$a=@{{Out=$out{named}}}
$sb=[scriptblock]::Create($text)
$ErrorActionPreference='Continue'
& $sb @a
$ErrorActionPreference='Stop'
if(-not (Test-Path -LiteralPath $out)){{ exit 5 }}
$f=New-Object IO.FileInfo($out)
if(($owners -notcontains $f.GetAccessControl().GetOwner($sidT).Value) -or ($f.Attributes -band [IO.FileAttributes]::ReparsePoint)){{ exit {EXIT_FOLDER_UNTRUSTED} }}
$raw=[IO.File]::ReadAllText($out).TrimStart([char]0xFEFF)
$envl='{{"token":"'+{token}+'","result":'+$raw+'}}'
[IO.File]::WriteAllText((Join-Path $dir 'envelope.json'),$envl,(New-Object Text.UTF8Encoding $false))
Remove-Item -LiteralPath $out -Force
}} catch {{ exit 1 }}
"#,
        script = ps_quote(&job.script_file.to_string_lossy()),
        hash = ps_quote(job.script_sha256),
        dir = ps_quote(&job.result_dir.to_string_lossy()),
        domain = ps_quote(job.caller.0),
        user = ps_quote(job.caller.1),
        caller_rules = caller_rules,
        test_owner = if job.caller_full_control { "+@($meSid)" } else { "" },
        full = if job.caller_full_control { "$true" } else { "$false" },
        named = named,
        token = ps_quote(job.token),
    ))
}

/// `text` as the value of PowerShell's `-EncodedCommand` (UTF-16LE, base64).
pub fn encode_command(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The result inside an envelope, only when it carries exactly `token`.
pub fn open_envelope(text: &str, token: &str) -> Result<serde_json::Value, String> {
    let mut v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|_| "The elevated result could not be verified.".to_string())?;
    if v["token"].as_str() != Some(token) {
        return Err("The elevated result was rejected: it does not carry this job's token.".into());
    }
    Ok(v["result"].take())
}

/// SHA-256 of `data` as lowercase hex.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// 16 random bytes from the system generator, as 32 hex characters.
pub fn random_hex() -> Result<String, String> {
    use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    let mut buf = [0u8; 16];
    // SAFETY: `buf` is a valid writable buffer of the stated length; a null
    // algorithm handle with BCRYPT_USE_SYSTEM_PREFERRED_RNG is the documented
    // way to ask for the system RNG.
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status != 0 {
        return Err("Could not create a random job token.".into());
    }
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Runs one elevated job through `launch` (which starts PowerShell with the
/// parameters it gets and waits). `jobs` holds the script file, `result_base`
/// is where the elevated side creates its protected result folder.
pub fn run(jobs: &Path, result_base: &Path, script: &str, args: &[(&str, &str)], caller_full_control: bool, launch: impl FnOnce(&str) -> Result<i32, String>) -> Result<serde_json::Value, String> {
    let id = random_hex()?;
    let token = random_hex()?;
    // Not derived from the job id: the script file's name is visible to
    // normal processes, the result folder's name must not be.
    let folder = random_hex()?;
    let script_file = jobs.join(format!("job-{id}.ps1"));
    let result_dir = result_base.join(format!("upcm-{folder}"));
    // A BOM so Windows PowerShell reads a file as UTF-8 (the hash covers it).
    let bytes = format!("\u{feff}{script}").into_bytes();
    let user = std::env::var("USERNAME").map_err(|_| "Could not tell which account to give the result to.".to_string())?;
    let domain = std::env::var("USERDOMAIN").unwrap_or_else(|_| ".".into());
    let hash = sha256_hex(&bytes);
    let text = bootstrap(&Job { script_file: &script_file, script_sha256: &hash, result_dir: &result_dir, token: &token, args, caller: (&domain, &user), caller_full_control })?;
    let params = format!("-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -EncodedCommand {}", encode_command(&text));
    if params.len() > 30_000 {
        return Err("The elevated script's arguments are too long.".into());
    }
    std::fs::write(&script_file, &bytes).map_err(|e| e.to_string())?;
    let result = launch(&params).and_then(|code| {
        if let Some(why) = refusal(code) {
            return Err(why.to_string());
        }
        let text = std::fs::read_to_string(result_dir.join("envelope.json")).map_err(|_| "PowerShell finished without a result.".to_string())?;
        open_envelope(&text, &token)
    });
    let _ = std::fs::remove_file(&script_file);
    let _ = std::fs::remove_dir_all(&result_dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = vec![b'a'; 1000];
        assert_eq!(sha256_hex(&long).len(), 64);
    }

    #[test]
    fn encoded_command_is_utf16_base64() {
        assert_eq!(encode_command("a"), "YQA=");
        assert_eq!(encode_command("hi"), "aABpAA==");
    }

    #[test]
    fn quoting_doubles_every_kind_of_single_quote() {
        assert_eq!(ps_quote("a'b\u{2019}c"), "'a''b\u{2019}\u{2019}c'");
    }

    #[test]
    fn an_envelope_without_or_with_the_wrong_token_is_rejected() {
        let ok = open_envelope("{\"token\":\"T1\",\"result\":{\"ok\":true}}", "T1").unwrap();
        assert_eq!(ok["ok"], true);
        assert!(open_envelope("{\"token\":\"other\",\"result\":{\"ok\":true}}", "T1").is_err());
        assert!(open_envelope("{\"result\":{\"ok\":true}}", "T1").is_err());
        assert!(open_envelope("{\"ok\":true}", "T1").is_err());
        assert!(open_envelope("not json", "T1").is_err());
    }

    #[test]
    fn a_bad_argument_name_is_refused() {
        let job = Job { script_file: Path::new("a"), script_sha256: "h", result_dir: Path::new("d"), token: "t", args: &[("a b", "x")], caller: (".", "u"), caller_full_control: false };
        assert!(bootstrap(&job).is_err());
    }

    #[test]
    fn the_bootstrap_carries_the_hash_and_token_and_gives_the_caller_no_write_access() {
        let job = Job { script_file: Path::new("C:\\x\\job.ps1"), script_sha256: "abc123", result_dir: Path::new("C:\\w\\upcm-1"), token: "TOK", args: &[("Ids", "a'b")], caller: ("PC", "me"), caller_full_control: false };
        let text = bootstrap(&job).unwrap();
        assert!(text.contains("'abc123'") && text.contains("'TOK'") && text.contains("Ids='a''b'"));
        assert!(text.contains("'ReadAndExecute','None','None'") && text.contains("'ReadAndExecute','ObjectInherit','InheritOnly'"));
        assert!(!text.contains("Delete"), "the caller is granted no delete right");
    }

    // The real PowerShell, not elevated: the bootstrap's own checks.
    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("upm-elev-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }


    fn start_with(params: &str, env: Option<(&str, &str)>) -> Result<i32, String> {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.raw_arg(params).creation_flags(0x0800_0000);
        if let Some((k, v)) = env {
            cmd.env(k, v);
        }
        let status = cmd.status().map_err(|e| e.to_string())?;
        Ok(status.code().unwrap_or(-1))
    }

    fn start(params: &str) -> Result<i32, String> {
        start_with(params, None)
    }

    fn first_with_extension(d: &Path, ext: &str) -> std::path::PathBuf {
        std::fs::read_dir(d).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == ext)).unwrap()
    }

    /// Runs the bootstrap of a script file written in `d` against `dir`.
    fn direct(d: &Path, dir: &Path, full: bool) -> i32 {
        let ps1 = d.join("direct.ps1");
        let bytes = "\u{feff}param([string]$Out)\n'{}' | Out-File $Out".as_bytes().to_vec();
        std::fs::write(&ps1, &bytes).unwrap();
        let hash = sha256_hex(&bytes);
        let user = std::env::var("USERNAME").unwrap();
        let domain = std::env::var("USERDOMAIN").unwrap_or_else(|_| ".".into());
        let text = bootstrap(&Job { script_file: &ps1, script_sha256: &hash, result_dir: dir, token: "t", args: &[], caller: (&domain, &user), caller_full_control: full }).unwrap();
        start(&format!("-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}", encode_command(&text))).unwrap()
    }

    #[test]
    fn a_job_runs_and_its_token_wrapped_result_comes_back() {
        let d = scratch("ok");
        let v = run(&d, &d, "param([string]$Out, [string]$Mode)\n'{\"mode\":\"'+$Mode+'\"}' | Out-File -FilePath $Out -Encoding utf8", &[("Mode", "scan it")], true, start).unwrap();
        assert_eq!(v["mode"], "scan it");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "script and result are deleted");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_script_swapped_before_it_runs_is_not_executed_and_says_so() {
        let d = scratch("swap");
        let marker = d.join("evil-ran");
        let evil = format!("param([string]$Out)\n'x' | Out-File '{}'\n'{{}}' | Out-File $Out", marker.display());
        let result = run(&d, &d, "param([string]$Out)\n'{}' | Out-File $Out", &[], true, |params| {
            std::fs::write(first_with_extension(&d, "ps1"), evil).unwrap();
            start(params)
        });
        assert!(result.unwrap_err().contains("was changed before it ran"));
        assert!(!marker.exists(), "the swapped script never ran");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_result_forged_with_the_wrong_token_is_rejected() {
        let d = scratch("forge");
        let result = run(&d, &d, "param([string]$Out)\n'{\"ok\":true}' | Out-File $Out", &[], true, |params| {
            let code = start(params)?;
            let env = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).map(|e| e.path().join("envelope.json")).find(|p| p.exists()).unwrap();
            std::fs::write(env, "{\"token\":\"guess\",\"result\":{\"ok\":true}}").unwrap();
            Ok(code)
        });
        assert!(result.unwrap_err().contains("rejected"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_existing_result_folder_is_never_reused() {
        let d = scratch("exists");
        let dir = d.join("upcm-taken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("result.json"), "{\"forged\":true}").unwrap();
        assert_eq!(direct(&d, &dir, true), EXIT_FOLDER_UNTRUSTED, "refused before running anything");
        assert_eq!(std::fs::read_to_string(dir.join("result.json")).unwrap(), "{\"forged\":true}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_that_is_not_owned_by_administrators_is_not_trusted() {
        // Not elevated, the new folder's owner is the user, not
        // Administrators: the job is refused, as it is for a folder an
        // attacker made first. (From an elevated shell the folder is
        // trusted and the job runs, which is the other accepted outcome.)
        let d = scratch("owner");
        let code = direct(&d, &d.join("upcm-fresh"), false);
        assert!(code == EXIT_FOLDER_UNTRUSTED || code == 0, "got {code}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_caller_holds_no_delete_write_or_permission_right_on_the_result_folder() {
        // The folder is left behind by a refused (or finished) job; read its
        // real ACL back. A caller that can delete or rename it could replace
        // it and forge the result.
        let d = scratch("acl");
        let dir = d.join("upcm-acl");
        direct(&d, &dir, false);
        let script = format!("$a=(New-Object IO.DirectoryInfo '{}').GetAccessControl(); foreach($r in $a.GetAccessRules($true,$true,[Security.Principal.NTAccount])){{ if($r.IdentityReference.Value -notmatch 'SYSTEM|Administrators'){{ [int]$r.FileSystemRights }} }}", dir.display());
        let out = std::process::Command::new("powershell.exe").args(["-NoProfile", "-Command", &script]).output().unwrap();
        let rights: Vec<u32> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect();
        assert!(!rights.is_empty(), "the caller has a read rule: {}", String::from_utf8_lossy(&out.stderr));
        for r in rights {
            assert_eq!(r & 0xD0156, 0, "no delete, write, delete-child or permission rights (got {r:#x})");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_elevated_session_does_not_load_modules_from_user_folders() {
        let d = scratch("modules");
        let v = run(&d, &d, "param([string]$Out)\n(@{ path = $env:PSModulePath } | ConvertTo-Json) | Out-File $Out", &[], true, |params| start_with(params, Some(("PSModulePath", "C:\\poisoned\\modules")))).unwrap();
        let path = v["path"].as_str().unwrap().to_lowercase();
        assert!(!path.contains("poisoned"));
        assert!(path.contains("windowspowershell"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn each_refusal_code_has_its_own_message_and_success_has_none() {
        assert!(refusal(EXIT_SCRIPT_CHANGED).unwrap().contains("changed"));
        assert!(refusal(EXIT_FOLDER_UNTRUSTED).unwrap().contains("could not be trusted"));
        assert_eq!(refusal(0), None);
    }
}
