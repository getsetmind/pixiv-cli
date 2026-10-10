use crate::{browser_cookies::SecretBytes, browser_secrets::BrowserSecretError};
use pixiv_sdk::context::RequestContext;
use std::{ffi::c_void, ptr, sync::Arc};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DpapiBlob {
    pub size: u32,
    pub data: *mut u8,
}

/// Native DPAPI and LocalFree dependency.
///
/// # Safety
/// Implementations must honor the Windows ABI and keep any returned nonnull output allocation
/// readable for its declared length until LocalFree. A successful nonempty output must satisfy
/// Rust slice allocation and length requirements. LocalFree must accept that exact allocation.
/// Input pointers are borrowed and must never be retained or freed.
/// The shared wrapper intentionally does not call LocalFree after native failure or a
/// zero-size result, preserving the frozen early-return allocation edges. Such native
/// allocations may remain unreleased; boundary replacements must safely own any residual
/// allocations without requiring a LocalFree call from the wrapper.
pub unsafe trait WindowsDpapiApi: Send + Sync {
    /// # Safety
    /// Blob storage and nonnull optional pointers must be valid for the native call. Input data
    /// must be readable for its declared length; output storage must be writable and initialized.
    #[allow(clippy::too_many_arguments)]
    unsafe fn crypt_unprotect_data(
        &self,
        input: *const DpapiBlob,
        description: *mut *mut u16,
        entropy: *const DpapiBlob,
        reserved: *mut c_void,
        prompt: *const c_void,
        flags: u32,
        output: *mut DpapiBlob,
    ) -> bool;

    /// # Safety
    /// `memory` must be a live allocation returned by this dependency, freed at most once.
    unsafe fn local_free(&self, memory: *mut c_void) -> *mut c_void;
}

pub struct WindowsDpapi {
    api: Arc<dyn WindowsDpapiApi>,
}

impl WindowsDpapi {
    pub fn new(api: Arc<dyn WindowsDpapiApi>) -> Self {
        Self { api }
    }

    pub fn system() -> Self {
        Self::new(Arc::new(SystemWindowsDpapiApi))
    }

    pub fn unprotect(
        &self,
        context: &dyn RequestContext,
        input: &[u8],
    ) -> Result<SecretBytes, BrowserSecretError> {
        if let Some(error) = context.error() {
            return Err(BrowserSecretError::Context(error));
        }
        let size = u32::try_from(input.len()).map_err(|_| BrowserSecretError::InvalidBlob)?;
        if size == 0 {
            return Err(BrowserSecretError::InvalidBlob);
        }
        let input_blob = DpapiBlob {
            size,
            data: input.as_ptr().cast_mut(),
        };
        let mut output = DpapiBlob {
            size: 0,
            data: ptr::null_mut(),
        };
        let succeeded = unsafe {
            self.api.crypt_unprotect_data(
                &input_blob,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                0,
                &mut output,
            )
        };
        if !succeeded || output.data.is_null() || output.size == 0 {
            return Err(BrowserSecretError::Dpapi);
        }
        let _allocation = Allocation {
            api: self.api.as_ref(),
            data: output.data,
        };
        let plain =
            unsafe { std::slice::from_raw_parts(output.data, output.size as usize) }.to_vec();
        if let Some(error) = context.error() {
            return Err(BrowserSecretError::Context(error));
        }
        Ok(SecretBytes::new(plain))
    }
}

struct Allocation<'a> {
    api: &'a dyn WindowsDpapiApi,
    data: *mut u8,
}

impl Drop for Allocation<'_> {
    fn drop(&mut self) {
        unsafe {
            self.api.local_free(self.data.cast());
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct SystemWindowsDpapiApi;

#[cfg(windows)]
const _: () = {
    use windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB;
    assert!(std::mem::size_of::<DpapiBlob>() == std::mem::size_of::<CRYPT_INTEGER_BLOB>());
    assert!(std::mem::align_of::<DpapiBlob>() == std::mem::align_of::<CRYPT_INTEGER_BLOB>());
    assert!(
        std::mem::offset_of!(DpapiBlob, size) == std::mem::offset_of!(CRYPT_INTEGER_BLOB, cbData)
    );
    assert!(
        std::mem::offset_of!(DpapiBlob, data) == std::mem::offset_of!(CRYPT_INTEGER_BLOB, pbData)
    );
};

#[cfg(windows)]
unsafe impl WindowsDpapiApi for SystemWindowsDpapiApi {
    unsafe fn crypt_unprotect_data(
        &self,
        input: *const DpapiBlob,
        description: *mut *mut u16,
        entropy: *const DpapiBlob,
        reserved: *mut c_void,
        prompt: *const c_void,
        flags: u32,
        output: *mut DpapiBlob,
    ) -> bool {
        unsafe {
            windows_sys::Win32::Security::Cryptography::CryptUnprotectData(
                input.cast(),
                description,
                entropy.cast(),
                reserved.cast_const(),
                prompt.cast(),
                flags,
                output.cast(),
            ) != 0
        }
    }

    unsafe fn local_free(&self, memory: *mut c_void) -> *mut c_void {
        unsafe { windows_sys::Win32::Foundation::LocalFree(memory) }
    }
}

#[cfg(not(windows))]
unsafe impl WindowsDpapiApi for SystemWindowsDpapiApi {
    unsafe fn crypt_unprotect_data(
        &self,
        _: *const DpapiBlob,
        _: *mut *mut u16,
        _: *const DpapiBlob,
        _: *mut c_void,
        _: *const c_void,
        _: u32,
        _: *mut DpapiBlob,
    ) -> bool {
        false
    }

    unsafe fn local_free(&self, memory: *mut c_void) -> *mut c_void {
        memory
    }
}
