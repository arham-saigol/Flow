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

fn sanitize_message(msg: &str) -> String {
    let mut safe = msg.to_string();
    if safe.contains("gsk_") || safe.contains("Bearer") || safe.contains("key") {
        safe = "[REDACTED]".into();
    }
    safe
}
