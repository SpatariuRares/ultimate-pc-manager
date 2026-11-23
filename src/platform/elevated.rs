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
    let caller_rights = if job.caller_full_control { "FullControl" } else { "ReadAndExecute, Delete, DeleteSubdirectoriesAndFiles" };
    Ok(format!(
        r#"$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
try {{
$bytes=[IO.File]::ReadAllBytes({script})
$sha=[Security.Cryptography.SHA256]::Create()
$h=([BitConverter]::ToString($sha.ComputeHash($bytes)) -replace '-','').ToLowerInvariant()
if($h -ne {hash}){{ exit 3 }}
$text=[Text.Encoding]::UTF8.GetString($bytes).TrimStart([char]0xFEFF)
$dir={dir}
if(Test-Path -LiteralPath $dir){{ exit 4 }}
$sec=New-Object Security.AccessControl.DirectorySecurity
$sec.SetAccessRuleProtection($true,$false)
foreach($sid in 'S-1-5-18','S-1-5-32-544'){{
  $id=New-Object Security.Principal.SecurityIdentifier($sid)
  $sec.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($id,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))
}}
$me=New-Object Security.Principal.NTAccount({domain},{user})
$sec.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($me,'{rights}','ContainerInherit,ObjectInherit','None','Allow')))
[void][IO.Directory]::CreateDirectory($dir,$sec)
$out=Join-Path $dir 'result.json'
$a=@{{Out=$out{named}}}
$sb=[scriptblock]::Create($text)
$ErrorActionPreference='Continue'
& $sb @a
$ErrorActionPreference='Stop'
if(-not (Test-Path -LiteralPath $out)){{ exit 5 }}
$raw=[IO.File]::ReadAllText($out).TrimStart([char]0xFEFF)
$envl='{{"token":"'+{token}+'","result":'+$raw+'}}'
[IO.File]::WriteAllText((Join-Path $dir 'envelope.json'),$envl,(New-Object Text.UTF8Encoding $false))
}} catch {{ exit 1 }}
"#,
        script = ps_quote(&job.script_file.to_string_lossy()),
        hash = ps_quote(job.script_sha256),
        dir = ps_quote(&job.result_dir.to_string_lossy()),
        domain = ps_quote(job.caller.0),
        user = ps_quote(job.caller.1),
        rights = caller_rights,
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
pub fn run(jobs: &Path, result_base: &Path, script: &str, args: &[(&str, &str)], caller_full_control: bool, launch: impl FnOnce(&str) -> Result<(), String>) -> Result<serde_json::Value, String> {
    let id = random_hex()?;
    let token = random_hex()?;
    let script_file = jobs.join(format!("job-{id}.ps1"));
    let result_dir = result_base.join(format!("upcm-{id}"));
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
    let result = launch(&params).and_then(|_| {
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
        assert!(text.contains("'ReadAndExecute, Delete, DeleteSubdirectoriesAndFiles'"));
    }

    // The real PowerShell, not elevated: the bootstrap's own checks.
    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("upm-elev-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn start(params: &str) -> Result<(), String> {
        super::super::imp::run_hidden(params, &std::sync::atomic::AtomicBool::new(false))
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
    fn a_script_swapped_before_it_runs_is_not_executed() {
        let d = scratch("swap");
        let marker = d.join("evil-ran");
        let evil = format!("param([string]$Out)\n'x' | Out-File '{}'\n'{{}}' | Out-File $Out", marker.display());
        let result = run(&d, &d, "param([string]$Out)\n'{}' | Out-File $Out", &[], true, |params| {
            let ps1 = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "ps1")).unwrap();
            std::fs::write(&ps1, evil).unwrap();
            start(params)
        });
        assert!(result.is_err());
        assert!(!marker.exists(), "the swapped script never ran");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_result_forged_with_the_wrong_token_is_rejected() {
        let d = scratch("forge");
        let result = run(&d, &d, "param([string]$Out)\n'{\"ok\":true}' | Out-File $Out", &[], true, |params| {
            start(params)?;
            let env = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).map(|e| e.path().join("envelope.json")).find(|p| p.exists()).unwrap();
            std::fs::write(env, "{\"token\":\"guess\",\"result\":{\"ok\":true}}").unwrap();
            Ok(())
        });
        assert!(result.unwrap_err().contains("rejected"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_existing_result_folder_is_never_reused() {
        let d = scratch("exists");
        let result = run(&d, &d, "param([string]$Out)\n'{}' | Out-File $Out", &[], true, |params| {
            // Someone pre-created the folder the elevated side is about to make.
            let ps1 = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "ps1")).unwrap();
            let id = ps1.file_stem().unwrap().to_string_lossy().trim_start_matches("job-").to_string();
            std::fs::create_dir_all(d.join(format!("upcm-{id}"))).unwrap();
            start(params)
        });
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
