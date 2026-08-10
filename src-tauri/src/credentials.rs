use windows::{
    core::{PCWSTR, PWSTR},
    Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    },
};

use crate::error::{FlowError, Result};

const GROQ_TARGET: &str = "Flow/GroqApiKey";
const DEEPGRAM_TARGET: &str = "Flow/DeepgramApiKey";

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn missing_api_key(provider: &str) -> FlowError {
    if provider == "Deepgram" {
        FlowError::MissingDeepgramApiKey
    } else {
        FlowError::MissingGroqApiKey
    }
}

fn has_api_key(target: &str, provider: &str) -> bool {
    read_api_key(target, provider)
        .map(|value| !value.is_empty())
        .unwrap_or(false)
}

fn read_api_key(target: &str, provider: &str) -> Result<String> {
    let target = wide(target);
    let mut credential = std::ptr::null_mut();
    unsafe {
        CredReadW(
            PCWSTR(target.as_ptr()),
            CRED_TYPE_GENERIC,
            0,
            &mut credential,
        )
        .map_err(|_| missing_api_key(provider))?;
        if credential.is_null() {
            return Err(missing_api_key(provider));
        }
        let record = &*credential;
        let bytes =
            std::slice::from_raw_parts(record.CredentialBlob, record.CredentialBlobSize as usize);
        let result = String::from_utf8(bytes.to_vec())
            .map_err(|_| FlowError::Message(format!("The saved {provider} API key is invalid.")));
        CredFree(credential.cast());
        result
    }
}

fn save_api_key(target: &str, api_key: &str, provider: &str) -> Result<()> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(FlowError::Message(format!(
            "The {provider} API key cannot be empty."
        )));
    }
    let mut target = wide(target);
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

fn delete_api_key(target: &str) -> Result<()> {
    let target = wide(target);
    unsafe {
        CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0).map_err(|error| {
            FlowError::Windows(format!("Could not restore the saved API key: {error}"))
        })
    }
}

pub fn has_groq_api_key() -> bool {
    has_api_key(GROQ_TARGET, "Groq")
}

pub fn read_groq_api_key() -> Result<String> {
    read_api_key(GROQ_TARGET, "Groq")
}

pub fn save_groq_api_key(api_key: &str) -> Result<()> {
    save_api_key(GROQ_TARGET, api_key, "Groq")
}

pub fn delete_groq_api_key() -> Result<()> {
    delete_api_key(GROQ_TARGET)
}

pub fn has_deepgram_api_key() -> bool {
    has_api_key(DEEPGRAM_TARGET, "Deepgram")
}

pub fn read_deepgram_api_key() -> Result<String> {
    read_api_key(DEEPGRAM_TARGET, "Deepgram")
}

pub fn save_deepgram_api_key(api_key: &str) -> Result<()> {
    save_api_key(DEEPGRAM_TARGET, api_key, "Deepgram")
}

pub fn delete_deepgram_api_key() -> Result<()> {
    delete_api_key(DEEPGRAM_TARGET)
}
