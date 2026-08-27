use std::{ffi::c_void, mem::size_of, os::windows::io::AsRawHandle};

use lazoo_runtime::OwnerSecurity;
use tokio::net::windows::named_pipe::NamedPipeServer;
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree},
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
                ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
                SDDL_REVISION_1, SE_KERNEL_OBJECT,
            },
            DACL_SECURITY_INFORMATION, GetTokenInformation, PSECURITY_DESCRIPTOR, RevertToSelf,
            SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::{
            Pipes::ImpersonateNamedPipeClient,
            Threading::{GetCurrentThread, OpenThreadToken},
        },
    },
    core::{HSTRING, PWSTR},
};

use crate::IpcError;

pub struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
    attributes: SECURITY_ATTRIBUTES,
}

impl PipeSecurity {
    pub fn new(owner: &OwnerSecurity) -> Result<Self, IpcError> {
        let sddl = HSTRING::from(owner.protected_sddl());
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .expect("SECURITY_ATTRIBUTES size exceeds u32"),
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        Ok(Self {
            descriptor,
            attributes,
        })
    }

    pub fn as_raw(&self) -> *mut c_void {
        (&raw const self.attributes).cast_mut().cast::<c_void>()
    }
}

unsafe impl Send for PipeSecurity {}
unsafe impl Sync for PipeSecurity {}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.descriptor.0)));
        }
    }
}

pub fn verify_client_owner(pipe: &NamedPipeServer, expected_sid: &str) -> Result<(), IpcError> {
    let handle = HANDLE(pipe.as_raw_handle());
    unsafe { ImpersonateNamedPipeClient(handle) }?;
    let _revert = RevertGuard;

    let mut token = HANDLE::default();
    unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &raw mut token) }?;
    let _token = HandleGuard(token);
    let actual = token_user_sid(token)?;
    if actual.eq_ignore_ascii_case(expected_sid) {
        Ok(())
    } else {
        Err(IpcError::Authentication)
    }
}

pub fn effective_sddl(pipe: &NamedPipeServer) -> Result<String, IpcError> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let code = unsafe {
        GetSecurityInfo(
            HANDLE(pipe.as_raw_handle()),
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&raw mut descriptor),
        )
    };
    if code != ERROR_SUCCESS {
        return Err(IpcError::WindowsCode(code.0));
    }
    let _descriptor = LocalGuard(HLOCAL(descriptor.0));
    let mut string = PWSTR::null();
    unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &raw mut string,
            None,
        )
    }?;
    let _string = LocalGuard(HLOCAL(string.0.cast::<c_void>()));
    Ok(unsafe { string.to_string() }?)
}

fn token_user_sid(token: HANDLE) -> Result<String, IpcError> {
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
    let mut string = PWSTR::null();
    unsafe { ConvertSidToStringSidW(token_user.User.Sid, &raw mut string) }?;
    let _string = LocalGuard(HLOCAL(string.0.cast::<c_void>()));
    Ok(unsafe { string.to_string() }?)
}

struct RevertGuard;

impl Drop for RevertGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = RevertToSelf();
        }
    }
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct LocalGuard(HLOCAL);

impl Drop for LocalGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(self.0));
        }
    }
}
