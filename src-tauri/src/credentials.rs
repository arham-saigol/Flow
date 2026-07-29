use windows::{
    core::{PCWSTR, PWSTR},
    Win32::Security::Credentials::{
        CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    },
};

use crate::error::{FlowError, Result};

const TARGET: &str = "Flow/GroqApiKey";

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn has_api_key() -> bool {
    read_api_key()
        .map(|value| !value.is_empty())
        .unwrap_or(false)
}

pub fn read_api_key() -> Result<String> {
    let target = wide(TARGET);
    let mut credential = std::ptr::null_mut();
    unsafe {
        CredReadW(
            PCWSTR(target.as_ptr()),
            CRED_TYPE_GENERIC,
            0,
            &mut credential,
        )
        .map_err(|_| FlowError::MissingApiKey)?;
        if credential.is_null() {
            return Err(FlowError::MissingApiKey);
        }
        let record = &*credential;
        let bytes =
            std::slice::from_raw_parts(record.CredentialBlob, record.CredentialBlobSize as usize);
        let result = String::from_utf8(bytes.to_vec())
            .map_err(|_| FlowError::Message("The saved Groq API key is invalid.".into()));
        CredFree(credential.cast());
        result
    }
}

pub fn save_api_key(api_key: &str) -> Result<()> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(FlowError::Message(
            "The Groq API key cannot be empty.".into(),
        ));
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
            FlowError::Windows(format!("Could not save the API key securely: {error}"))
        })
    }
}
