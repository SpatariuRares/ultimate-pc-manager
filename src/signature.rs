//! Who signed a program file: WinVerifyTrust on the file's own signature or,
//! for the many Windows files signed only through a catalog, on the catalog
//! entry for its hash. Checked on a worker thread, once per path, so the UI
//! and the sampler never wait for it. "Hide Microsoft Services and
//! Processes" reads which files carry a verified Microsoft Windows
//! signature; a Publisher column can read the signer's name from the same
//! cache.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::path_cache::PathCache;

/// A verified signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// The signer's name as Windows shows it ("Microsoft Windows").
    pub signer: String,
    /// The signer is Microsoft Windows and its chain ends at a Microsoft root.
    pub microsoft: bool,
}

/// Whether a signer is Microsoft Windows: its certificate's name is
/// "Microsoft Windows" or "Microsoft Windows Publisher" and its chain ends
/// at a Microsoft root, so a look-alike name under another root is not.
pub fn is_microsoft_windows(signer: &str, microsoft_root: bool) -> bool {
    microsoft_root && matches!(signer, "Microsoft Windows" | "Microsoft Windows Publisher")
}

/// How a file is checked: the real WinVerifyTrust, or a fake in tests.
type Check = fn(&str) -> Option<Signature>;

/// The signature cache: ask for paths, read what is known so far. Each
/// path's verified signature, or None for none, checked on a worker
/// thread ([`PathCache`]).
pub struct Signatures {
    cache: PathCache<Option<Signature>>,
    /// The paths with a verified Microsoft Windows signature, rebuilt when
    /// the cache has grown so readers take it without copying: (the
    /// cache's generation it was built from, the set).
    microsoft: Mutex<(u64, Arc<HashSet<String>>)>,
}

impl Default for Signatures {
    fn default() -> Self {
        Signatures::with_check(verify)
    }
}

impl Signatures {
    fn with_check(check: Check) -> Self {
        Signatures { cache: PathCache::new("signatures", check), microsoft: Mutex::new((0, Arc::default())) }
    }

    /// A cache that already knows `microsoft` as Microsoft Windows signed
    /// and checks nothing else (tests).
    #[cfg(test)]
    pub fn known(microsoft: &[&str]) -> Self {
        let s = Signatures::with_check(|_| None);
        let mut f = s.cache.found();
        for path in microsoft {
            f.by_path.insert(path.to_string(), Some(Signature { signer: "Microsoft Windows".into(), microsoft: true }));
        }
        f.generation += 1;
        drop(f);
        s
    }

    /// Queues the paths not checked or asked for yet; returns at once.
    pub fn ask<'a>(&self, paths: impl IntoIterator<Item = &'a str>) {
        self.cache.ask(paths);
    }

    /// The paths known to carry a verified Microsoft Windows signature.
    pub fn microsoft(&self) -> Arc<HashSet<String>> {
        let f = self.cache.found();
        let mut built = crate::platform::lock_state(&self.microsoft);
        if built.0 != f.generation {
            let set = f.by_path.iter().filter(|(_, sig)| sig.as_ref().is_some_and(|s| s.microsoft)).map(|(path, _)| path.clone()).collect();
            *built = (f.generation, Arc::new(set));
        }
        Arc::clone(&built.1)
    }

    /// The signature of `path`: None while it is not checked yet,
    /// Some(None) for a file without a verified signature.
    pub fn of(&self, path: &str) -> Option<Option<Signature>> {
        self.cache.of(path)
    }
}

/// What the Publisher column shows for a checked file: its verified
/// signer, or "Not verified" for a file without a verified signature.
pub fn publisher(checked: Option<Signature>) -> String {
    checked.map_or_else(|| NOT_VERIFIED.to_string(), |s| s.signer)
}

/// The Publisher column's text for a file without a verified signature.
pub const NOT_VERIFIED: &str = "Not verified";

/// The verified signature of the file at `path`, if any: its own
/// (embedded) signature, else a catalog's.
#[cfg(windows)]
pub fn verify(path: &str) -> Option<Signature> {
    let wide = crate::win_ffi::wide(path);
    // SAFETY: `wide` is a NUL-terminated path that outlives both calls.
    unsafe { win::embedded(&wide).or_else(|| win::catalog(&wide)) }
}

#[cfg(not(windows))]
pub fn verify(_path: &str) -> Option<Signature> {
    None
}

#[cfg(windows)]
mod win {
    use super::{is_microsoft_windows, Signature};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Cryptography::Catalog::{
        CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash, CryptCATAdminReleaseCatalogContext,
        CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext, CATALOG_INFO,
    };
    use windows_sys::Win32::Security::Cryptography::{
        CertGetNameStringW, CertVerifyCertificateChainPolicy, CERT_CHAIN_POLICY_MICROSOFT_ROOT, CERT_CHAIN_POLICY_PARA, CERT_CHAIN_POLICY_STATUS,
        CERT_NAME_SIMPLE_DISPLAY_TYPE,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2,
        WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE,
        WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};

    /// Runs WinVerifyTrust on `data` (its union already set) and, when the
    /// signature verifies, reads the signer.
    unsafe fn trust(mut data: WINTRUST_DATA) -> Option<Signature> {
        data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        // No network: revocation and URL retrieval stay off.
        data.dwProvFlags = WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let ok = WinVerifyTrust(null_mut(), &mut action, &mut data as *mut _ as *mut _) == 0;
        let sig = if ok { signer(&data) } else { None };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(null_mut(), &mut action, &mut data as *mut _ as *mut _);
        sig
    }

    /// The first signer's name and whether its chain ends at a Microsoft root.
    unsafe fn signer(data: &WINTRUST_DATA) -> Option<Signature> {
        let prov = WTHelperProvDataFromStateData(data.hWVTStateData);
        if prov.is_null() {
            return None;
        }
        let sgnr = WTHelperGetProvSignerFromChain(prov, 0, 0, 0);
        if sgnr.is_null() {
            return None;
        }
        let cert = WTHelperGetProvCertFromChain(sgnr, 0);
        if cert.is_null() || (*cert).pCert.is_null() {
            return None;
        }
        let mut buf = [0u16; 256];
        let n = CertGetNameStringW((*cert).pCert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null(), buf.as_mut_ptr(), buf.len() as u32);
        let name = String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)]);
        let chain = (*sgnr).pChainContext;
        let root = !chain.is_null() && {
            let para = CERT_CHAIN_POLICY_PARA {
                cbSize: std::mem::size_of::<CERT_CHAIN_POLICY_PARA>() as u32,
                // No flags: Microsoft's product roots (Windows signs under
                // "Microsoft Root Certificate Authority 2010").
                dwFlags: 0,
                pvExtraPolicyPara: null_mut(),
            };
            let mut status = CERT_CHAIN_POLICY_STATUS { cbSize: std::mem::size_of::<CERT_CHAIN_POLICY_STATUS>() as u32, ..Default::default() };
            CertVerifyCertificateChainPolicy(CERT_CHAIN_POLICY_MICROSOFT_ROOT, chain, &para, &mut status) != 0 && status.dwError == 0
        };
        Some(Signature { microsoft: is_microsoft_windows(&name, root), signer: name })
    }

    /// The file's own signature.
    pub unsafe fn embedded(path: &[u16]) -> Option<Signature> {
        let mut file = WINTRUST_FILE_INFO { cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32, pcwszFilePath: path.as_ptr(), ..Default::default() };
        let mut data = WINTRUST_DATA { dwUnionChoice: WTD_CHOICE_FILE, ..Default::default() };
        data.Anonymous.pFile = &mut file;
        trust(data)
    }

    /// The signature of the catalog that lists the file's hash (SHA-256,
    /// then SHA-1 for older catalogs).
    pub unsafe fn catalog(path: &[u16]) -> Option<Signature> {
        let handle = CreateFileW(path.as_ptr(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, null(), OPEN_EXISTING, 0, null_mut());
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut sig = None;
        for algorithm in ["SHA256", "SHA1"] {
            let alg = crate::win_ffi::wide(algorithm);
            let mut admin = 0isize;
            if CryptCATAdminAcquireContext2(&mut admin, null(), alg.as_ptr(), null(), 0) == 0 {
                continue;
            }
            let mut size = 0u32;
            CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut size, null_mut(), 0);
            let mut hash = vec![0u8; size as usize];
            if size > 0 && CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut size, hash.as_mut_ptr(), 0) != 0 {
                let cat = CryptCATAdminEnumCatalogFromHash(admin, hash.as_ptr(), size, 0, null_mut());
                if cat != 0 {
                    let mut info = CATALOG_INFO { cbStruct: std::mem::size_of::<CATALOG_INFO>() as u32, ..Default::default() };
                    if CryptCATCatalogInfoFromContext(cat, &mut info, 0) != 0 {
                        // The member tag is the hash in upper-case hex.
                        let tag = crate::win_ffi::wide(&hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
                        let mut member = WINTRUST_CATALOG_INFO {
                            cbStruct: std::mem::size_of::<WINTRUST_CATALOG_INFO>() as u32,
                            pcwszCatalogFilePath: info.wszCatalogFile.as_ptr(),
                            pcwszMemberTag: tag.as_ptr(),
                            pcwszMemberFilePath: path.as_ptr(),
                            hMemberFile: handle,
                            pbCalculatedFileHash: hash.as_mut_ptr(),
                            cbCalculatedFileHash: size,
                            hCatAdmin: admin,
                            ..Default::default()
                        };
                        let mut data = WINTRUST_DATA { dwUnionChoice: WTD_CHOICE_CATALOG, ..Default::default() };
                        data.Anonymous.pCatalog = &mut member;
                        sig = trust(data);
                    }
                    CryptCATAdminReleaseCatalogContext(admin, cat, 0);
                }
            }
            CryptCATAdminReleaseContext(admin, 0);
            if sig.is_some() {
                break;
            }
        }
        CloseHandle(handle);
        sig
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn only_a_microsoft_windows_signer_under_a_microsoft_root_counts() {
        assert!(is_microsoft_windows("Microsoft Windows", true));
        assert!(is_microsoft_windows("Microsoft Windows Publisher", true));
        assert!(!is_microsoft_windows("Microsoft Windows", false), "a look-alike under another root");
        assert!(!is_microsoft_windows("Valve Corp.", true));
        assert!(!is_microsoft_windows("Microsoft Corporation", true), "Microsoft's other software is not Windows");
    }

    /// A check that takes a while, like WinVerifyTrust on a large file.
    fn slow(path: &str) -> Option<Signature> {
        std::thread::sleep(Duration::from_millis(200));
        (path == "ms.exe").then(|| Signature { signer: "Microsoft Windows".into(), microsoft: true })
    }

    #[test]
    fn asking_returns_at_once_and_the_answers_arrive_from_the_worker() {
        let s = Signatures::with_check(slow);
        let start = Instant::now();
        s.ask(["ms.exe", "other.exe", "ms.exe", ""]);
        assert!(start.elapsed() < Duration::from_millis(100), "asking does not wait for the checks");
        assert_eq!(s.of("ms.exe"), None, "not checked yet");
        let deadline = Instant::now() + Duration::from_secs(5);
        while s.of("other.exe").is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(s.of("other.exe"), Some(None), "checked: no verified signature");
        assert_eq!(s.of("ms.exe").flatten().map(|s| s.signer), Some("Microsoft Windows".to_string()));
        assert_eq!(*s.microsoft(), HashSet::from(["ms.exe".to_string()]));
    }

    #[cfg(windows)]
    #[test]
    fn the_command_prompt_is_microsoft_windows_signed_and_the_tests_are_not() {
        let own = std::env::current_exe().expect("own path");
        assert!(!verify(&own.to_string_lossy()).is_some_and(|s| s.microsoft), "the test binary is not Microsoft's");
        let cmd = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        let sig = verify(&cmd).expect("cmd.exe carries a verified signature");
        assert!(sig.microsoft, "{sig:?}");
    }
}
