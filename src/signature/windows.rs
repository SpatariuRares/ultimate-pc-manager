//! Windows: WinVerifyTrust on the file's own signature, else on its
//! catalog entry.

use crate::signature::{is_microsoft_windows, Signature};
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

/// The verified signature of the file at `path`, if any: its own
/// (embedded) signature, else a catalog's.
pub fn verify(path: &str) -> Option<Signature> {
    let wide = crate::win_ffi::wide(path);
    // SAFETY: `wide` is a NUL-terminated path that outlives both calls.
    unsafe { embedded(&wide).or_else(|| catalog(&wide)) }
}
