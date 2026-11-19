//! Native Authenticode validation. Pin the leaf certificate from the very same
//! WinVerifyTrust state that validated the file, never a separately parsed
//! certificate or the timestamp countersigner.

use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Security::Cryptography::{
    CertGetCertificateContextProperty, CERT_SHA256_HASH_PROP_ID,
};
use windows::Win32::Security::WinTrust::{
    WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData,
    WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
    WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
    WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
};

struct TrustState(WINTRUST_DATA);

impl Drop for TrustState {
    fn drop(&mut self) {
        self.0.dwStateAction = WTD_STATEACTION_CLOSE;
        // SAFETY: data and its file-info/path/handle remain alive until this
        // guard is dropped. CLOSE releases state even after a failed VERIFY.
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        unsafe {
            WinVerifyTrust(HWND(-1isize as _), &mut action, &mut self.0 as *mut _ as _);
        }
    }
}

pub(super) fn verify_publisher(path: &Path, file: &File, pins: &[[u8; 32]]) -> Result<(), String> {
    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide_path.as_ptr()),
        hFile: HANDLE(file.as_raw_handle()),
        ..Default::default()
    };
    let mut state = TrustState(WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
        ..Default::default()
    });
    // SAFETY: all pointers/handles refer to live objects; Windows owns the
    // verification state, which the guard closes after signer extraction.
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status =
        unsafe { WinVerifyTrust(HWND(-1isize as _), &mut action, &mut state.0 as *mut _ as _) };
    if status != 0 {
        return super::signature::windows_signer_allowed(status, None, pins);
    }
    // SAFETY: provider/signer/certificate pointers are owned by the live
    // successful WinVerifyTrust state. Check each for NULL before dereferencing.
    let thumbprint = unsafe {
        let provider = WTHelperProvDataFromStateData(state.0.hWVTStateData);
        if provider.is_null() {
            return Err("Authenticode verification returned no provider state".into());
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
        if signer.is_null() {
            return Err("Authenticode verification returned no primary signer".into());
        }
        let certificate = WTHelperGetProvCertFromChain(signer, 0);
        let certificate = certificate
            .as_ref()
            .filter(|certificate| !certificate.pCert.is_null())
            .ok_or_else(|| "Authenticode verification returned no leaf certificate".to_string())?;
        let mut digest = [0u8; 32];
        let mut length = digest.len() as u32;
        CertGetCertificateContextProperty(
            certificate.pCert,
            CERT_SHA256_HASH_PROP_ID,
            Some(digest.as_mut_ptr().cast()),
            &mut length,
        )
        .map_err(|e| format!("Authenticode certificate SHA-256: {e}"))?;
        if length != digest.len() as u32 {
            return Err("Authenticode certificate returned an invalid SHA-256 length".into());
        }
        digest
    };
    super::signature::windows_signer_allowed(status, Some(&thumbprint), pins)
}
