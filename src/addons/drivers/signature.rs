//! Driver signatures: who signed a driver file, and whether it is WHQL.
//!
//! The check is `WinVerifyTrust` with the driver policy, first on the
//! file's own (embedded) signature, then on the catalog that lists the
//! file's hash (`CryptCATAdmin*`), because most drivers are signed through
//! their package's catalog. It needs no administrator rights. WHQL is
//! decided from the signer chain by [`verdict`].

use std::collections::HashMap;

/// The signer of a driver Microsoft's hardware compatibility program
/// (WHQL) tested: when it is in the signer chain, the driver is WHQL.
pub const WHQL_PUBLISHER: &str = "Microsoft Windows Hardware Compatibility Publisher";

/// What was found about one driver file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signature {
    /// Signed, and the signer chain has [`WHQL_PUBLISHER`].
    Whql { signer: String },
    /// Signed by someone else.
    Signed { signer: String },
    /// Neither the file nor a catalog carries a signature.
    Unsigned,
    /// The check itself failed: this says why, and it is not "Unsigned".
    CouldNotCheck(String),
}

impl Signature {
    /// What the column and the Signature row show.
    pub fn label(&self) -> String {
        match self {
            Signature::Whql { .. } => "WHQL".into(),
            Signature::Signed { .. } => "Signed".into(),
            Signature::Unsigned => "Unsigned".into(),
            Signature::CouldNotCheck(reason) => format!("Could not check: {reason}"),
        }
    }

    /// The signer's name, when there is one.
    pub fn signer(&self) -> Option<&str> {
        match self {
            Signature::Whql { signer } | Signature::Signed { signer } if !signer.is_empty() => Some(signer),
            _ => None,
        }
    }
}

/// The text of the Signature column and row for a file: its result, or
/// "Not checked" while there is none.
pub fn signature_text(found: Option<&Signature>) -> String {
    found.map_or_else(|| "Not checked".to_string(), Signature::label)
}

/// The verdict for a signature that verified: `chain` holds the names of the
/// certificates from the signer up to the root. An empty chain has no
/// signer, so it is unsigned; a chain with [`WHQL_PUBLISHER`] anywhere in it
/// is WHQL. The signer shown is the first name.
pub fn verdict(chain: &[String]) -> Signature {
    let Some(signer) = chain.first() else { return Signature::Unsigned };
    let signer = signer.clone();
    if chain.iter().any(|name| name.trim().eq_ignore_ascii_case(WHQL_PUBLISHER)) {
        Signature::Whql { signer }
    } else {
        Signature::Signed { signer }
    }
}

/// What the checks found, by file path (case does not matter). The hub
/// replaces the whole book when it grows, so the page holds a cheap
/// snapshot and can tell when it changed.
#[derive(Clone, Debug, Default)]
pub struct SignatureBook(HashMap<String, Signature>);

impl SignatureBook {
    fn key(path: &str) -> String {
        path.trim().to_lowercase()
    }

    /// The result for the file at `path`; None while it is not checked.
    pub fn of(&self, path: &str) -> Option<&Signature> {
        self.0.get(&Self::key(path))
    }

    pub fn insert(&mut self, path: &str, signature: Signature) {
        self.0.insert(Self::key(path), signature);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The signature of the driver file at `path`, with normal rights.
#[cfg(windows)]
pub fn check(path: &str) -> Signature {
    win::check(path)
}

#[cfg(not(windows))]
pub fn check(_path: &str) -> Signature {
    Signature::CouldNotCheck("signatures are checked on Windows only".into())
}

#[cfg(windows)]
mod win {
    use super::{verdict, Signature};
    use std::ptr::{null, null_mut};
    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Cryptography::Catalog::{
        CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash, CryptCATAdminReleaseCatalogContext,
        CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext, CATALOG_INFO,
    };
    use windows_sys::Win32::Security::Cryptography::{CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE};
    use windows_sys::Win32::Security::WinTrust::{
        WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_CATALOG_INFO, WINTRUST_DATA,
        WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
        WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};

    /// The driver policy of WinVerifyTrust (`DRIVER_ACTION_VERIFY`).
    const DRIVER_ACTION_VERIFY: GUID = GUID { data1: 0xF750E6C3, data2: 0x38EE, data3: 0x11D1, data4: [0x85, 0xE5, 0x00, 0xC0, 0x4F, 0xC2, 0x95, 0xEE] };

    /// The codes WinVerifyTrust answers for a file nobody signed.
    const NOT_SIGNED: [u32; 3] = [
        0x800B0100, // TRUST_E_NOSIGNATURE
        0x800B0003, // TRUST_E_SUBJECT_FORM_UNKNOWN
        0x800B0001, // TRUST_E_PROVIDER_UNKNOWN
    ];

    fn is_unsigned(code: i32) -> bool {
        NOT_SIGNED.contains(&(code as u32))
    }

    /// The reason a code is told as: Windows' own text for it.
    fn reason(code: i32) -> String {
        let text = std::io::Error::from_raw_os_error(code).to_string();
        // "… (os error N)" is noise for the user.
        let text = text.split(" (os error").next().unwrap_or(&text).trim().to_string();
        if text.is_empty() || text.starts_with("Unknown error") {
            format!("error 0x{:08X}", code as u32)
        } else {
            text
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn check(path: &str) -> Signature {
        let wide = wide(path);
        // SAFETY: `wide` is a NUL-terminated path that outlives the calls.
        unsafe {
            let embedded = match trust_file(&wide) {
                Ok(chain) => return verdict(&chain),
                Err(code) => code,
            };
            // A file that cannot be opened has no catalog to look for.
            if !is_unsigned(embedded) && !std::path::Path::new(path).is_file() {
                return Signature::CouldNotCheck(reason(embedded));
            }
            match trust_catalog(&wide) {
                Ok(Some(chain)) => verdict(&chain),
                Ok(None) if is_unsigned(embedded) => Signature::Unsigned,
                Ok(None) => Signature::CouldNotCheck(reason(embedded)),
                Err(code) if is_unsigned(code) => Signature::Unsigned,
                Err(code) => Signature::CouldNotCheck(reason(code)),
            }
        }
    }

    /// Runs WinVerifyTrust with the driver policy on `data` (its union
    /// already set): the signer chain when it verifies, else the code.
    unsafe fn trust(mut data: WINTRUST_DATA) -> Result<Vec<String>, i32> {
        data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        // No network: revocation and URL retrieval stay off.
        data.dwProvFlags = WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL;
        let mut action = DRIVER_ACTION_VERIFY;
        let code = WinVerifyTrust(null_mut(), &mut action, &mut data as *mut _ as *mut _);
        let chain = if code == 0 { signer_chain(&data) } else { Vec::new() };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(null_mut(), &mut action, &mut data as *mut _ as *mut _);
        if code == 0 {
            Ok(chain)
        } else {
            Err(code)
        }
    }

    /// The names of the first signer's certificates, signer first.
    unsafe fn signer_chain(data: &WINTRUST_DATA) -> Vec<String> {
        let mut names = Vec::new();
        let prov = WTHelperProvDataFromStateData(data.hWVTStateData);
        if prov.is_null() {
            return names;
        }
        let signer = WTHelperGetProvSignerFromChain(prov, 0, 0, 0);
        if signer.is_null() {
            return names;
        }
        for i in 0..(*signer).csCertChain {
            let cert = WTHelperGetProvCertFromChain(signer, i);
            if cert.is_null() || (*cert).pCert.is_null() {
                continue;
            }
            let mut buf = [0u16; 256];
            let n = CertGetNameStringW((*cert).pCert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null(), buf.as_mut_ptr(), buf.len() as u32);
            names.push(String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)]));
        }
        names
    }

    /// The file's own signature.
    unsafe fn trust_file(path: &[u16]) -> Result<Vec<String>, i32> {
        let mut file = WINTRUST_FILE_INFO { cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32, pcwszFilePath: path.as_ptr(), ..Default::default() };
        let mut data = WINTRUST_DATA { dwUnionChoice: WTD_CHOICE_FILE, ..Default::default() };
        data.Anonymous.pFile = &mut file;
        trust(data)
    }

    /// The signature of the catalog that lists the file's hash (SHA-256,
    /// then SHA-1 for older catalogs): Ok(None) when no catalog lists it.
    unsafe fn trust_catalog(path: &[u16]) -> Result<Option<Vec<String>>, i32> {
        let handle = CreateFileW(path.as_ptr(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, null(), OPEN_EXISTING, 0, null_mut());
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(2));
        }
        let mut found = Ok(None);
        for algorithm in ["SHA256", "SHA1"] {
            let alg = wide(algorithm);
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
                        let tag = wide(&hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
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
                        found = trust(data).map(Some);
                    }
                    CryptCATAdminReleaseCatalogContext(admin, cat, 0);
                }
            }
            CryptCATAdminReleaseContext(admin, 0);
            if matches!(found, Ok(Some(_))) {
                break;
            }
        }
        CloseHandle(handle);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(chain: &[&str]) -> Vec<String> {
        chain.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_chain_with_the_hardware_compatibility_publisher_is_whql() {
        let chain = names(&[WHQL_PUBLISHER, "Microsoft Windows Third Party Component CA 2014", "Microsoft Root Authority"]);
        assert_eq!(verdict(&chain), Signature::Whql { signer: WHQL_PUBLISHER.into() });
    }

    #[test]
    fn the_publisher_anywhere_in_the_chain_makes_it_whql_and_the_signer_is_the_first_name() {
        let chain = names(&["Realtek Semiconductor Corp.", WHQL_PUBLISHER, "Microsoft Root Authority"]);
        assert_eq!(verdict(&chain), Signature::Whql { signer: "Realtek Semiconductor Corp.".into() });
        let lower = names(&["microsoft windows hardware compatibility publisher"]);
        assert!(matches!(verdict(&lower), Signature::Whql { .. }), "case does not matter");
    }

    #[test]
    fn any_other_chain_is_signed_with_its_signer() {
        let chain = names(&["WireGuard LLC", "DigiCert Trusted G4 Code Signing", "DigiCert Trusted Root G4"]);
        assert_eq!(verdict(&chain), Signature::Signed { signer: "WireGuard LLC".into() });
        // A look-alike name is not the publisher.
        let fake = names(&["Microsoft Windows Hardware Compatibility Publisher 2", "Some Root"]);
        assert!(matches!(verdict(&fake), Signature::Signed { .. }));
    }

    #[test]
    fn a_chain_without_a_signer_is_unsigned() {
        assert_eq!(verdict(&[]), Signature::Unsigned);
    }

    #[test]
    fn the_shown_values_are_fixed_and_a_failed_check_is_not_unsigned() {
        let whql = Signature::Whql { signer: WHQL_PUBLISHER.into() };
        let signed = Signature::Signed { signer: "Acme".into() };
        let failed = Signature::CouldNotCheck("the file is in use".into());
        assert_eq!(whql.label(), "WHQL");
        assert_eq!(signed.label(), "Signed");
        assert_eq!(Signature::Unsigned.label(), "Unsigned");
        assert_eq!(failed.label(), "Could not check: the file is in use");
        assert_ne!(failed.label(), Signature::Unsigned.label());
        assert_eq!(signature_text(None), "Not checked");
        assert_eq!(signature_text(Some(&signed)), "Signed");
        assert_eq!(whql.signer(), Some(WHQL_PUBLISHER));
        assert_eq!(signed.signer(), Some("Acme"));
        assert_eq!(Signature::Unsigned.signer(), None);
        assert_eq!(failed.signer(), None);
    }

    #[test]
    fn the_book_finds_a_path_whatever_its_case() {
        let mut book = SignatureBook::default();
        assert!(book.is_empty());
        book.insert(r"C:\Windows\System32\Drivers\Disk.sys", Signature::Unsigned);
        assert_eq!(book.of(r"c:\windows\system32\drivers\disk.SYS"), Some(&Signature::Unsigned));
        assert_eq!(book.of(r"C:\other.sys"), None);
        assert_eq!(book.len(), 1);
    }

    /// Catalog-signed Windows drivers have no embedded signature: they must
    /// not be called unsigned. The test binary is signed by nobody.
    #[cfg(windows)]
    #[test]
    #[ignore = "checks this PC's driver files"]
    fn this_pc_does_not_call_catalog_signed_drivers_unsigned() {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        for file in ["disk.sys", "ntfs.sys", "tcpip.sys"] {
            let path = format!(r"{windir}\System32\drivers\{file}");
            let found = check(&path);
            println!("{file}: {found:?}");
            assert!(matches!(found, Signature::Signed { .. } | Signature::Whql { .. }), "{file} is signed through a catalog, not {found:?}");
        }
        let own = std::env::current_exe().unwrap();
        let found = check(&own.to_string_lossy());
        println!("test binary: {found:?}");
        assert_eq!(found, Signature::Unsigned, "a file nobody signed");
        let missing = check(&format!(r"{windir}\System32\drivers\no-such-driver.sys"));
        println!("missing: {missing:?}");
        assert!(matches!(missing, Signature::CouldNotCheck(_)), "a missing file is a failed check, not Unsigned");
    }

    /// Every kernel driver of this PC gets a result, and most are signed.
    #[cfg(windows)]
    #[test]
    #[ignore = "checks this PC's driver files"]
    fn this_pc_checks_every_kernel_driver() {
        use crate::addons::drivers::ListingReader;
        let list = crate::addons::drivers::windows::WindowsListing.kernel().expect("kernel drivers");
        let (mut whql, mut signed, mut unsigned, mut failed) = (0, 0, 0, 0);
        for d in list.iter().filter(|d| std::path::Path::new(&d.path).is_file()) {
            match check(&d.path) {
                Signature::Whql { .. } => whql += 1,
                Signature::Signed { .. } => signed += 1,
                Signature::Unsigned => {
                    println!("unsigned: {}", d.path);
                    unsigned += 1
                }
                Signature::CouldNotCheck(why) => {
                    println!("could not check {}: {why}", d.path);
                    failed += 1
                }
            }
        }
        println!("{whql} WHQL, {signed} signed, {unsigned} unsigned, {failed} could not be checked");
        assert!(whql + signed > unsigned + failed, "most of this PC's drivers are signed");
    }
}
