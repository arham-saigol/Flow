use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_LOG_SIZE: u64 = 1_048_576; // 1 MiB
pub const MAX_LOG_FILES: usize = 3;

static LOGGER: Mutex<Option<DiagnosticsLogger>> = Mutex::new(None);

pub struct DiagnosticsLogger {
    log_dir: PathBuf,
    current_file: PathBuf,
}

impl DiagnosticsLogger {
    pub fn init(log_dir: &Path) {
        let _ = fs::create_dir_all(log_dir);
        let current_file = log_dir.join("flow.log");
        let mut logger = LOGGER.lock().unwrap();
        *logger = Some(Self {
            log_dir: log_dir.to_path_buf(),
            current_file,
        });
    }

    pub fn log(stage: &str, session_id: Option<u64>, error_code: Option<&str>, message: &str) {
        if let Ok(mut lock) = LOGGER.lock() {
            if let Some(logger) = lock.as_mut() {
                logger.write_entry(stage, session_id, error_code, message);
            }
        }
    }

    fn write_entry(
        &mut self,
        stage: &str,
        session_id: Option<u64>,
        error_code: Option<&str>,
        message: &str,
    ) {
        self.rotate_if_needed();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Sanitize message to prevent logging credentials, headers, bodies
        let safe_msg = sanitize_message(message);

        let line = format!(
            "[{now}] stage={stage} session={:?} code={:?} msg={safe_msg}\n",
            session_id, error_code
        );

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.current_file)
        {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    fn rotate_if_needed(&self) {
        if let Ok(meta) = fs::metadata(&self.current_file) {
            if meta.len() >= MAX_LOG_SIZE {
                for i in (1..MAX_LOG_FILES).rev() {
                    let from = self.log_dir.join(format!("flow.{i}.log"));
                    let to = self.log_dir.join(format!("flow.{}.log", i + 1));
                    let _ = fs::rename(from, to);
                }
                let backup = self.log_dir.join("flow.1.log");
                let _ = fs::rename(&self.current_file, backup);
            }
        }
    }

    pub fn export(&self) -> String {
        let mut full_log = String::new();
        for i in (1..=MAX_LOG_FILES).rev() {
            let path = self.log_dir.join(format!("flow.{i}.log"));
            if let Ok(content) = fs::read_to_string(path) {
                full_log.push_str(&content);
            }
        }
        if let Ok(content) = fs::read_to_string(&self.current_file) {
            full_log.push_str(&content);
        }
        full_log
    }
}

pub fn log_event(stage: &str, session_id: Option<u64>, error_code: Option<&str>, message: &str) {
    DiagnosticsLogger::log(stage, session_id, error_code, message);
}

pub fn export_diagnostics() -> String {
    if let Ok(lock) = LOGGER.lock() {
        if let Some(logger) = lock.as_ref() {
            return logger.export();
        }
    }
    String::new()
}

fn find_case_insensitive(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    // ASCII-only needles, searched without changing the haystack so byte
    // offsets always stay valid char boundaries even for non-ASCII text.
    let hay = haystack.as_bytes();
    let ned = needle.as_bytes();
    let mut start = from;
    while start + ned.len() <= hay.len() {
        if hay[start..start + ned.len()].eq_ignore_ascii_case(ned) {
            return Some(start);
        }
        start += 1;
    }
    None
}

fn redact_secret_tokens(
    msg: &str,
    needle: &str,
    keep_prefix: usize,
    token_len: impl Fn(&str) -> usize,
) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut cursor = 0;
    while let Some(match_start) = find_case_insensitive(msg, needle, cursor) {
        out.push_str(&msg[cursor..match_start + keep_prefix]);
        let token_end =
            (match_start + needle.len() + token_len(&msg[match_start + needle.len()..]))
                .min(msg.len());
        out.push_str("[REDACTED]");
        cursor = token_end;
    }
    out.push_str(&msg[cursor..]);
    out
}

fn sanitize_message(msg: &str) -> String {
    // Case-insensitive, token-specific secret detection. Ordinary words such
    // as "hotkey" or "keyboard" no longer trigger redaction; only the matched
    // secret token is replaced and the rest of the message is preserved.
    let with_keys_redacted = redact_secret_tokens(msg, "gsk_", 0, |rest| {
        rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(rest.len())
    });
    redact_secret_tokens(&with_keys_redacted, "bearer", "bearer".len(), |rest| {
        let leading = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let after = &rest[leading..];
        leading + after.find(char::is_whitespace).unwrap_or(after.len())
    })
}

#[cfg(test)]
mod tests {
    use super::sanitize_message;

    #[test]
    fn ordinary_words_are_not_redacted() {
        assert_eq!(
            sanitize_message("hotkey captured: keyboard F12"),
            "hotkey captured: keyboard F12"
        );
    }

    #[test]
    fn secret_tokens_are_redacted_in_place() {
        assert_eq!(
            sanitize_message("request failed for key Gsk_AbC-123_xY with status 401"),
            "request failed for key [REDACTED] with status 401"
        );
        assert_eq!(
            sanitize_message("Authorization: Bearer sk-secret123 rejected"),
            "Authorization: Bearer[REDACTED] rejected"
        );
        assert_eq!(
            sanitize_message("authorization: bearer\ttok.en here"),
            "authorization: bearer[REDACTED] here"
        );
    }

    #[test]
    fn non_ascii_text_does_not_break_offset_slicing() {
        assert_eq!(
            sanitize_message("İstanbul bearer abc123"),
            "İstanbul bearer[REDACTED]"
        );
    }
}
