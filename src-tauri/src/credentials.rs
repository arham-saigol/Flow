use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{GetLastError, WIN32_ERROR},
        Security::Credentials::{
            CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
            CRED_TYPE_GENERIC,
        },
    },
};

use crate::error::{FlowError, Result};

const TARGET: &str = "Flow/GroqApiKey";
const MAX_CREDENTIAL_BLOB_BYTES: usize = 2560; // 2,560 bytes (CRED_MAX_CREDENTIAL_BLOB_SIZE)

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

struct CredGuard(*mut CREDENTIALW);

impl Drop for CredGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CredFree(self.0.cast());
            }
        }
    }
}

pub fn has_api_key() -> bool {
    matches!(read_api_key(), Ok(ref value) if !value.is_empty())
}

pub fn read_api_key() -> Result<String> {
    let target = wide(TARGET);
    let mut credential = std::ptr::null_mut();

    unsafe {
        let ok = CredReadW(
            PCWSTR(target.as_ptr()),
            CRED_TYPE_GENERIC,
            0,
            &mut credential,
        );

        if let Err(error) = ok {
            let code = GetLastError();
            // 1168 is ERROR_NOT_FOUND
            if code == WIN32_ERROR(1168) {
                return Err(FlowError::MissingApiKey);
            }
            return Err(FlowError::Windows(format!(
                "Could not access Windows Credential Manager: {error}"
            )));
        }

        let _guard = CredGuard(credential);

        if credential.is_null() {
            return Err(FlowError::MissingApiKey);
        }

        let record = &*credential;
        if record.CredentialBlob.is_null() || record.CredentialBlobSize == 0 {
            return Err(FlowError::MissingApiKey);
        }

        let size = record.CredentialBlobSize as usize;
        if size > MAX_CREDENTIAL_BLOB_BYTES {
            return Err(FlowError::Message(
                "The stored API key is corrupt or too large.".into(),
            ));
        }

        let bytes = std::slice::from_raw_parts(record.CredentialBlob, size);
        let key_str = std::str::from_utf8(bytes)
            .map_err(|_| FlowError::Message("The stored API key is not valid UTF-8.".into()))?
            .trim();

        if key_str.is_empty() {
            return Err(FlowError::MissingApiKey);
        }

        if key_str.chars().any(|c| c.is_control()) {
            return Err(FlowError::Message(
                "The stored API key contains invalid characters.".into(),
            ));
        }

        Ok(key_str.to_string())
    }
}

pub fn save_api_key(api_key: &str) -> Result<()> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(FlowError::Message(
            "The Groq API key cannot be empty.".into(),
        ));
    }
    if key.chars().any(|c| c.is_control()) {
        return Err(FlowError::Message(
            "The Groq API key cannot contain control characters.".into(),
        ));
    }
    if key.len() > MAX_CREDENTIAL_BLOB_BYTES {
        return Err(FlowError::Message("The Groq API key is too long.".into()));
    }

    let mut target = wide(TARGET);
    let mut username = wide("Flow");
    let mut blob = key.as_bytes().to_vec();

    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(username.as_mut_ptr()),
        ..Default::default()
    };

    unsafe {
        CredWriteW(&credential, 0).map_err(|error| {
            FlowError::Windows(format!(
                "Could not save API key to Windows Credential Manager: {error}"
            ))
        })
    }
}

pub fn delete_api_key() -> Result<()> {
    let target = wide(TARGET);
    unsafe {
        if let Err(error) = CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0) {
            let code = GetLastError();
            // 1168 is ERROR_NOT_FOUND - deleting a non-existent key is a success
            if code != WIN32_ERROR(1168) {
                return Err(FlowError::Windows(format!(
                    "Could not remove API key from Windows Credential Manager: {error}"
                )));
            }
        }
    }
    Ok(())
}
