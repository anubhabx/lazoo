use std::{ffi::c_void, path::Path, slice};

use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            Cryptography::{
                CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
            },
            DACL_SECURITY_INFORMATION, GetFileSecurityW, GetTokenInformation,
            PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SetFileSecurityW,
            TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    },
    core::{HSTRING, PWSTR},
};

use crate::RuntimeError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerSecurity {
    sid: String,
}

impl OwnerSecurity {
    pub fn current() -> Result<Self, RuntimeError> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }?;
        let token_guard = HandleGuard(token);

        let mut required = 0_u32;
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &raw mut required) };
        let words = (required as usize).div_ceil(size_of::<usize>());
        let mut buffer = vec![0_usize; words];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast::<c_void>()),
                required,
                &raw mut required,
            )
        }?;

        let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut sid_string = PWSTR::null();
        unsafe { ConvertSidToStringSidW(token_user.User.Sid, &raw mut sid_string) }?;
        let sid = unsafe { sid_string.to_string() }?;
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sid_string.0.cast::<c_void>())));
        }
        drop(token_guard);

        Ok(Self { sid })
    }

    #[must_use]
    pub fn sid(&self) -> &str {
        &self.sid
    }

    #[must_use]
    pub fn protected_sddl(&self) -> String {
        format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", self.sid)
    }

    pub fn apply_to_path(&self, path: &Path) -> Result<(), RuntimeError> {
        let sddl = HSTRING::from(self.protected_sddl());
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }?;

        let path = HSTRING::from(path.to_string_lossy().as_ref());
        let result = unsafe {
            SetFileSecurityW(
                &path,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            )
            .ok()
        };
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        result.map_err(RuntimeError::Windows)
    }

    pub fn read_path_sddl(path: &Path) -> Result<String, RuntimeError> {
        let path = HSTRING::from(path.to_string_lossy().as_ref());
        let flags = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        let mut required = 0_u32;
        let _ = unsafe { GetFileSecurityW(&path, flags.0, None, 0, &raw mut required) };
        let words = (required as usize).div_ceil(size_of::<usize>());
        let mut buffer = vec![0_usize; words];
        let descriptor = PSECURITY_DESCRIPTOR(buffer.as_mut_ptr().cast::<c_void>());
        unsafe {
            GetFileSecurityW(
                &path,
                flags.0,
                Some(descriptor),
                required,
                &raw mut required,
            )
            .ok()
        }?;

        let mut string = PWSTR::null();
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                flags,
                &raw mut string,
                None,
            )
        }?;
        let sddl = unsafe { string.to_string() }?;
        unsafe {
            let _ = LocalFree(Some(HLOCAL(string.0.cast::<c_void>())));
        }
        Ok(sddl)
    }
}

pub fn protect_for_current_user(data: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    crypt_data(data, true)
}

pub fn unprotect_for_current_user(data: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    crypt_data(data, false)
}

fn crypt_data(data: &[u8], protect: bool) -> Result<Vec<u8>, RuntimeError> {
    if data.is_empty() {
        return Err(RuntimeError::EmptyProtectedData);
    }

    let mut input = data.to_vec();
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input.len()).expect("protected input exceeds u32"),
        pbData: input.as_mut_ptr(),
    };
    let mut output_blob = CRYPT_INTEGER_BLOB::default();

    if protect {
        unsafe {
            CryptProtectData(
                &raw const input_blob,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output_blob,
            )
        }?;
    } else {
        unsafe {
            CryptUnprotectData(
                &raw const input_blob,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output_blob,
            )
        }?;
    }

    let output =
        unsafe { slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(output_blob.pbData.cast::<c_void>())));
    }
    Ok(output)
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
