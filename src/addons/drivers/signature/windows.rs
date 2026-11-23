//! Windows: `WinVerifyTrust` with the driver policy, on the file and on its catalog.

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
