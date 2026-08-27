use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE},
        System::Threading::{CreateMutexW, ReleaseMutex},
    },
    core::HSTRING,
};

use crate::RuntimeError;

#[derive(Debug)]
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    pub fn acquire(
        installation_id: uuid::Uuid,
        role: impl AsRef<str>,
    ) -> Result<Self, RuntimeError> {
        let role = role.as_ref();
        let name = HSTRING::from(format!(
            r"Local\Lazoo-{}-{}",
            installation_id.simple(),
            role
        ));
        let handle = unsafe { CreateMutexW(None, true, &name) }?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(RuntimeError::AlreadyRunning {
                role: role.to_owned(),
            });
        }

        Ok(Self { handle })
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}
