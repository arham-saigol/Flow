use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::{FlowError, Result},
    models::{
        AppConfig, DashboardData, DictionaryEntry, HistoryEntry, HistoryRetention, Keybind,
        PendingDictation, SettingsData, Snippet, MAX_DICTIONARY_CORRECTION_CHARS,
        MAX_DICTIONARY_ENTRIES, MAX_DICTIONARY_SOURCE_CHARS, MAX_RECOVERY_BYTES,
        MAX_RECOVERY_ITEMS, MAX_SNIPPETS, MAX_SNIPPET_CONTENT_CHARS, MAX_SNIPPET_TRIGGER_CHARS,
        RECOVERY_RETENTION_DAYS,
    },
    text,
};

pub const WAV_RESERVATION_BYTES: u64 = 9_600_044; // 5 minutes 16kHz mono 16-bit PCM WAV
pub const MARGIN_BYTES: u64 = 1_048_576; // 1 MiB
pub const PRE_CAPTURE_RESERVATION_BYTES: u64 = 2 * WAV_RESERVATION_BYTES + MARGIN_BYTES; // 20,248,664 bytes
pub const MIN_VOLUME_FREE_BYTES_START: u64 =
    4 * WAV_RESERVATION_BYTES + MARGIN_BYTES + 64 * 1024 * 1024; // 106,557,616 bytes
#[allow(dead_code)]
pub const MIN_VOLUME_FREE_BYTES_IMPORT: u64 =
    2 * WAV_RESERVATION_BYTES + MARGIN_BYTES + 64 * 1024 * 1024; // 86,309,024 bytes

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    pub created_at: i64,
    pub expires_at: i64,
    pub original_version: i32,
    pub backup_file: String,
}

pub struct PendingDictationRecord {
    pub id: i64,
    pub capture_uuid: Option<String>,
    pub wav: Option<Vec<u8>>,
    pub raw_text: Option<String>,
    pub corrected_text: Option<String>,
    pub final_text: Option<String>,
    pub duration_ms: i64,
    pub partial: bool,
    pub review_reason: Option<String>,
    pub no_content: bool,
    pub delivery_mode: String,
    pub delivery_outcome: String,
    pub delivery_warning: Option<String>,
    pub history_saved: bool,
    pub history_id: Option<i64>,
}

pub struct Database {
    connection: Mutex<Connection>,
    path: PathBuf,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                FlowError::Message(format!("Could not create directory for database: {e}"))
            })?;
        }

        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;

        connection.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = FULL;
            PRAGMA foreign_keys = ON;
            PRAGMA secure_delete = ON;
            PRAGMA busy_timeout = 5000;
            ",
        )?;

        let current_version: i32 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;

        if current_version > 2 {
            return Err(FlowError::Message(
                "The local database was created by a newer version of Flow. Please update Flow to open it.".into(),
            ));
        }

        let table_count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        let is_existing_db = table_count > 0;

        if current_version == 0 {
            // Adopt or create baseline schema
            connection.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    raw_text TEXT NOT NULL,
                    word_count INTEGER NOT NULL,
                    duration_ms INTEGER NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS history_created_at ON history(created_at DESC);
                CREATE TABLE IF NOT EXISTS aggregate_stats (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    total_words INTEGER NOT NULL,
                    total_duration_ms INTEGER NOT NULL
                );
                INSERT OR IGNORE INTO aggregate_stats(id, total_words, total_duration_ms)
                SELECT 1, COALESCE(SUM(word_count), 0), COALESCE(SUM(duration_ms), 0)
                FROM history;
                CREATE TABLE IF NOT EXISTS weekly_stats (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    word_count INTEGER NOT NULL,
                    duration_ms INTEGER NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS weekly_stats_created_at ON weekly_stats(created_at);
                INSERT INTO weekly_stats(word_count, duration_ms, created_at)
                SELECT word_count, duration_ms, created_at FROM history
                WHERE NOT EXISTS (SELECT 1 FROM weekly_stats);
                CREATE TABLE IF NOT EXISTS pending_dictations (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    wav BLOB,
                    raw_text TEXT,
                    final_text TEXT,
                    duration_ms INTEGER NOT NULL,
                    history_saved INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS dictionary (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    value TEXT NOT NULL COLLATE NOCASE UNIQUE,
                    correction TEXT,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS snippets (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    trigger TEXT NOT NULL COLLATE NOCASE UNIQUE,
                    content TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                ",
            )?;

            let has_correction: bool = connection.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM pragma_table_info('dictionary')
                    WHERE name = 'correction'
                )",
                [],
                |row| row.get(0),
            )?;
            if !has_correction {
                connection.execute("ALTER TABLE dictionary ADD COLUMN correction TEXT", [])?;
            }

            connection.execute("PRAGMA user_version = 1", [])?;
        }

        let updated_version: i32 =
            connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;

        if updated_version == 1 {
            // Backup before structural migration if this was an existing
            // database. A failed backup aborts the migration so the version-1
            // database is left untouched for the next launch to retry.
            if is_existing_db {
                Self::create_pre_upgrade_backup(&connection, path, 1)?;
            }

            // Run migration 2 in a transaction
            let tx = connection.transaction()?;
            tx.execute_batch(
                "
                ALTER TABLE pending_dictations ADD COLUMN capture_uuid TEXT;
                ALTER TABLE pending_dictations ADD COLUMN corrected_text TEXT;
                ALTER TABLE pending_dictations ADD COLUMN partial INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE pending_dictations ADD COLUMN review_reason TEXT;
                ALTER TABLE pending_dictations ADD COLUMN no_content INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE pending_dictations ADD COLUMN delivery_outcome TEXT NOT NULL DEFAULT 'not_attempted';
                ALTER TABLE pending_dictations ADD COLUMN delivery_warning TEXT;
                ALTER TABLE pending_dictations ADD COLUMN error_code TEXT;
                ALTER TABLE pending_dictations ADD COLUMN retry_after INTEGER;
                ALTER TABLE pending_dictations ADD COLUMN history_id INTEGER;
                ALTER TABLE history ADD COLUMN capture_uuid TEXT;
                ALTER TABLE history ADD COLUMN delivery_outcome TEXT NOT NULL DEFAULT 'unknown';
                ALTER TABLE history ADD COLUMN delivery_warning TEXT;
                ALTER TABLE snippets ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
                ALTER TABLE snippets ADD COLUMN conflict_reason TEXT;
                ALTER TABLE dictionary ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
                ALTER TABLE dictionary ADD COLUMN conflict_reason TEXT;
                ALTER TABLE pending_dictations ADD COLUMN delivery_mode TEXT NOT NULL DEFAULT 'copy_only';
                CREATE UNIQUE INDEX IF NOT EXISTS pending_capture_uuid ON pending_dictations(capture_uuid);
                CREATE UNIQUE INDEX IF NOT EXISTS history_capture_uuid ON history(capture_uuid);
                CREATE INDEX IF NOT EXISTS pending_created_id ON pending_dictations(created_at DESC, id DESC);
                CREATE INDEX IF NOT EXISTS history_created_id ON history(created_at DESC, id DESC);
                ",
            )?;

            // Assign UUIDs to existing pending rows
            let mut stmt =
                tx.prepare("SELECT id FROM pending_dictations WHERE capture_uuid IS NULL")?;
            let ids: Vec<i64> = stmt
                .query_map([], |row| row.get(0))?
                .filter_map(std::result::Result::ok)
                .collect();
            drop(stmt);

            for id in ids {
                let uuid_str = Uuid::new_v4().to_string();
                tx.execute(
                    "UPDATE pending_dictations SET capture_uuid = ?1 WHERE id = ?2",
                    params![uuid_str, id],
                )?;
            }

            // F15: Group existing dictionary corrections by normalized source
            let mut dict_stmt = tx.prepare(
                "SELECT id, value, correction FROM dictionary WHERE correction IS NOT NULL AND trim(correction) != ''",
            )?;
            let dict_rows: Vec<(i64, String, String)> = dict_stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .filter_map(std::result::Result::ok)
                .collect();
            drop(dict_stmt);

            let mut dict_groups: HashMap<String, Vec<i64>> = HashMap::new();
            for (id, val, _) in dict_rows {
                let norm = text::normalize_key(&val);
                if !norm.is_empty() {
                    dict_groups.entry(norm).or_default().push(id);
                }
            }

            for (_, group_ids) in dict_groups {
                if group_ids.len() > 1 {
                    for id in group_ids {
                        tx.execute(
                            "UPDATE dictionary SET enabled = 0, conflict_reason = 'normalized_source_conflict' WHERE id = ?1",
                            params![id],
                        )?;
                    }
                }
            }

            Self::recompute_dictionary_conflicts(&tx)?;

            // F16: Group existing snippets by normalized trigger
            let mut snip_stmt = tx.prepare("SELECT id, trigger FROM snippets")?;
            let snip_rows: Vec<(i64, String)> = snip_stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .filter_map(std::result::Result::ok)
                .collect();
            drop(snip_stmt);

            let mut snip_groups: HashMap<String, Vec<i64>> = HashMap::new();
            let mut invalid_snips: Vec<i64> = Vec::new();
            for (id, trigger) in snip_rows {
                if let Some(norm) = text::normalize_snippet_trigger(&trigger) {
                    snip_groups.entry(norm).or_default().push(id);
                } else {
                    invalid_snips.push(id);
                }
            }

            for id in invalid_snips {
                tx.execute(
                    "UPDATE snippets SET enabled = 0, conflict_reason = 'normalized_trigger_conflict' WHERE id = ?1",
                    params![id],
                )?;
            }

            for (_, group_ids) in snip_groups {
                if group_ids.len() > 1 {
                    for id in group_ids {
                        tx.execute(
                            "UPDATE snippets SET enabled = 0, conflict_reason = 'normalized_trigger_conflict' WHERE id = ?1",
                            params![id],
                        )?;
                    }
                }
            }

            tx.execute("PRAGMA user_version = 2", [])?;
            tx.commit()?;
        }

        Ok(Self {
            connection: Mutex::new(connection),
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn create_pre_upgrade_backup(
        conn: &Connection,
        db_path: &Path,
        original_version: i32,
    ) -> Result<()> {
        let parent = db_path.parent().unwrap_or(Path::new("."));
        let free_space = get_available_disk_space(parent)?;
        let min_backup_free = 4 * WAV_RESERVATION_BYTES + MARGIN_BYTES + 64 * 1024 * 1024;
        if free_space < min_backup_free {
            return Err(FlowError::QuotaExceeded(
                "Not enough free disk space to create pre-upgrade backup.".into(),
            ));
        }

        let backups_dir = parent.join("backups");
        std::fs::create_dir_all(&backups_dir)
            .map_err(|e| FlowError::Message(format!("Could not create backups folder: {e}")))?;

        let now = Self::now();
        let backup_file_name = format!("flow_backup_v{original_version}_{now}.sqlite3");
        let backup_path = backups_dir.join(&backup_file_name);

        let mut dst = match Connection::open(&backup_path) {
            Ok(connection) => connection,
            Err(error) => {
                let _ = std::fs::remove_file(&backup_path);
                return Err(FlowError::from(error));
            }
        };

        // Every failure past this point leaves a partial or unusable backup
        // file behind; remove it before returning the error so no corrupt
        // backup is kept and the next launch can retry from a clean state.
        let result: Result<()> = (|| {
            let backup = rusqlite::backup::Backup::new(conn, &mut dst)?;
            let busy_timeout = Duration::from_secs(10);
            let mut busy_start: Option<std::time::Instant> = None;

            loop {
                match backup.step(100) {
                    Ok(rusqlite::backup::StepResult::Done) => break,
                    Ok(rusqlite::backup::StepResult::More) => {
                        busy_start = None;
                    }
                    Ok(rusqlite::backup::StepResult::Busy)
                    | Ok(rusqlite::backup::StepResult::Locked) => {
                        let bstart = busy_start.get_or_insert_with(std::time::Instant::now);
                        if bstart.elapsed() > busy_timeout {
                            return Err(FlowError::Message(
                                "Database busy timeout exceeded during backup".into(),
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Ok(_) => {
                        busy_start = None;
                    }
                    Err(e) => return Err(FlowError::from(e)),
                }
            }

            let metadata = BackupMetadata {
                created_at: now,
                expires_at: now + 7 * 86_400,
                original_version,
                backup_file: backup_file_name,
            };

            let meta_json = serde_json::to_string_pretty(&metadata).map_err(|e| {
                FlowError::Message(format!("Could not serialize backup metadata: {e}"))
            })?;
            std::fs::write(backups_dir.join("backup_metadata.json"), meta_json)
                .map_err(|e| FlowError::Message(format!("Could not save backup metadata: {e}")))?;

            Ok(())
        })();

        // Close the destination connection before unlinking so the removal
        // is not blocked by an open file handle on Windows.
        drop(dst);
        if result.is_err() {
            let _ = std::fs::remove_file(&backup_path);
        }
        result
    }

    pub fn get_backup_info(&self) -> Option<(String, i64)> {
        let parent = self.path.parent().unwrap_or(Path::new("."));
        let backups_dir = parent.join("backups");
        let meta_path = backups_dir.join("backup_metadata.json");
        if meta_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<BackupMetadata>(&content) {
                    return Some((meta.backup_file, meta.expires_at));
                }
            }
        }
        None
    }

    pub fn delete_upgrade_backup(&self) -> Result<()> {
        let parent = self.path.parent().unwrap_or(Path::new("."));
        let backups_dir = parent.join("backups");
        let meta_path = backups_dir.join("backup_metadata.json");
        if meta_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<BackupMetadata>(&content) {
                    let backup_file = backups_dir.join(&meta.backup_file);
                    let _ = std::fs::remove_file(backup_file);
                }
            }
            let _ = std::fs::remove_file(meta_path);
        }
        Ok(())
    }

    pub fn prune_expired_backup(&self) -> Result<()> {
        let parent = self.path.parent().unwrap_or(Path::new("."));
        let backups_dir = parent.join("backups");
        let meta_path = backups_dir.join("backup_metadata.json");
        if meta_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<BackupMetadata>(&content) {
                    if Self::now() >= meta.expires_at {
                        let backup_file = backups_dir.join(&meta.backup_file);
                        let _ = std::fs::remove_file(backup_file);
                        let _ = std::fs::remove_file(meta_path);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| FlowError::Message("The local database is unavailable.".into()))
    }

    pub fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    pub fn wal_checkpoint(&self) -> Result<()> {
        self.conn()?
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()?
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    fn put_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
        conn.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn settings(&self, has_api_key: bool) -> Result<SettingsData> {
        let defaults = SettingsData::default();
        let privacy_version = self
            .setting("privacy_notice_version")?
            .and_then(|v| v.parse::<i64>().ok());

        Ok(SettingsData {
            has_api_key,
            microphone_id: self
                .setting("microphone_id")?
                .unwrap_or(defaults.microphone_id),
            microphone_name: self
                .setting("microphone_name")?
                .unwrap_or(defaults.microphone_name),
            keybind: self.setting("keybind")?.unwrap_or(defaults.keybind),
            launch_at_startup: self
                .setting("launch_at_startup")?
                .map(|value| value == "true")
                .unwrap_or(defaults.launch_at_startup),
            history_retention: self
                .setting("history_retention")?
                .unwrap_or(defaults.history_retention),
            privacy_notice_version: privacy_version,
        })
    }

    /// Validates settings values (keybind, history retention) without writing.
    /// This is the validation path used before any settings save side effects.
    pub fn validate_settings(&self, settings: &SettingsData) -> Result<()> {
        if settings.keybind.parse::<Keybind>().is_err() {
            return Err(FlowError::Message("Unsupported keybind.".into()));
        }
        if settings
            .history_retention
            .parse::<HistoryRetention>()
            .is_err()
        {
            return Err(FlowError::Message("Unsupported retention period.".into()));
        }
        Ok(())
    }

    pub fn save_settings(&self, settings: &SettingsData) -> Result<()> {
        // Validate keybind and retention
        self.validate_settings(settings)?;

        let mut conn = self.conn()?;
        let transaction = conn.transaction()?;
        Self::put_setting(&transaction, "microphone_id", &settings.microphone_id)?;
        Self::put_setting(&transaction, "microphone_name", &settings.microphone_name)?;
        Self::put_setting(&transaction, "keybind", &settings.keybind)?;
        Self::put_setting(
            &transaction,
            "launch_at_startup",
            if settings.launch_at_startup {
                "true"
            } else {
                "false"
            },
        )?;
        Self::put_setting(
            &transaction,
            "history_retention",
            &settings.history_retention,
        )?;
        if let Some(v) = settings.privacy_notice_version {
            Self::put_setting(&transaction, "privacy_notice_version", &v.to_string())?;
        }
        if let Some(seconds) = retention_seconds(&settings.history_retention) {
            let cutoff = Self::now() - seconds;
            transaction.execute(
                "UPDATE pending_dictations SET history_id = NULL WHERE history_id IN (SELECT id FROM history WHERE created_at < ?1)",
                [cutoff],
            )?;
            transaction.execute("DELETE FROM history WHERE created_at < ?1", [cutoff])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn set_privacy_notice_acknowledged(&self, version: i64) -> Result<()> {
        let conn = self.conn()?;
        Self::put_setting(&conn, "privacy_notice_version", &version.to_string())
    }

    pub fn prune_expired_history(&self, retention: &str) -> Result<usize> {
        if let Some(seconds) = retention_seconds(retention) {
            let mut conn = self.conn()?;
            let tx = conn.transaction()?;
            let cutoff = Self::now() - seconds;
            tx.execute(
                "UPDATE pending_dictations SET history_id = NULL WHERE history_id IN (SELECT id FROM history WHERE created_at < ?1)",
                [cutoff],
            )?;
            let deleted = tx.execute("DELETE FROM history WHERE created_at < ?1", [cutoff])?;
            tx.commit()?;
            Ok(deleted)
        } else {
            Ok(0)
        }
    }

    pub fn prune_weekly_stats(&self) -> Result<usize> {
        let cutoff = Self::now() - 7 * 86_400;
        let deleted = self
            .conn()?
            .execute("DELETE FROM weekly_stats WHERE created_at < ?1", [cutoff])?;
        Ok(deleted)
    }

    pub fn prune_expired_pending(
        &self,
        active_pending_id: Option<i64>,
    ) -> Result<Vec<(i64, Option<String>)>> {
        let cutoff = Self::now() - RECOVERY_RETENTION_DAYS as i64 * 86_400;
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT id, capture_uuid FROM pending_dictations WHERE created_at < ?1")?;
        let expired: Vec<(i64, Option<String>)> = stmt
            .query_map([cutoff], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(std::result::Result::ok)
            .filter(|(id, _)| Some(*id) != active_pending_id)
            .collect();
        drop(stmt);

        for (id, _) in &expired {
            conn.execute("DELETE FROM pending_dictations WHERE id = ?1", [id])?;
        }
        Ok(expired)
    }

    pub fn check_recovery_quota(&self, spool_bytes: u64, spool_uuids: usize) -> Result<()> {
        let conn = self.conn()?;
        let (db_pending_count, db_pending_wav_bytes): (i64, i64) = conn.query_row(
            "SELECT count(*), COALESCE(SUM(length(wav)), 0) FROM pending_dictations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let total_items = (db_pending_count as usize) + spool_uuids;
        if total_items + 1 > MAX_RECOVERY_ITEMS {
            return Err(FlowError::QuotaExceeded(format!(
                "Maximum {MAX_RECOVERY_ITEMS} recovery items reached. Please copy or discard existing dictations."
            )));
        }

        let total_bytes = (db_pending_wav_bytes as u64) + spool_bytes;
        if total_bytes + PRE_CAPTURE_RESERVATION_BYTES > MAX_RECOVERY_BYTES {
            return Err(FlowError::QuotaExceeded(
                "Recovery storage quota (256 MB) reached. Please copy or discard existing dictations.".into(),
            ));
        }

        let free_space = get_available_disk_space(&self.path)?;
        if free_space < MIN_VOLUME_FREE_BYTES_START {
            return Err(FlowError::QuotaExceeded(
                "Not enough free disk space to start recording.".into(),
            ));
        }

        Ok(())
    }

    pub fn run_maintenance(&self, active_pending_id: Option<i64>) -> Result<()> {
        let retention = self
            .setting("history_retention")?
            .unwrap_or_else(|| SettingsData::default().history_retention);
        let _ = self.prune_expired_history(&retention);
        let _ = self.prune_weekly_stats();
        let _ = self.prune_expired_pending(active_pending_id);
        let _ = self.prune_expired_backup();
        Ok(())
    }

    pub fn dashboard(&self) -> Result<DashboardData> {
        let retention = self
            .setting("history_retention")?
            .unwrap_or_else(|| SettingsData::default().history_retention);
        let _ = self.prune_expired_history(&retention);
        let _ = self.prune_weekly_stats();

        let conn = self.conn()?;
        let (total_words, total_duration): (i64, i64) = conn.query_row(
            "SELECT total_words, total_duration_ms FROM aggregate_stats WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (weekly_words, weekly_duration): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(word_count), 0), COALESCE(SUM(duration_ms), 0) FROM weekly_stats",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let history = self.history_page_internal(&conn, 50, None, None)?;
        let pending = self.pending_dictations_internal(&conn)?;

        let average_words_per_minute = if total_duration > 0 {
            total_words
                .saturating_mul(60_000)
                .saturating_add(total_duration / 2)
                / total_duration
        } else {
            0
        };
        const TYPING_MS_PER_WORD: i64 = 1_500;
        let typing_ms = weekly_words.saturating_mul(TYPING_MS_PER_WORD);

        Ok(DashboardData {
            total_words_dictated: total_words,
            average_words_per_minute,
            time_dictated_ms: weekly_duration,
            estimated_saved_ms: typing_ms.saturating_sub(weekly_duration),
            history,
            pending,
        })
    }

    pub fn history_page(
        &self,
        limit: usize,
        before_created_at: Option<i64>,
        before_id: Option<i64>,
    ) -> Result<Vec<HistoryEntry>> {
        let conn = self.conn()?;
        self.history_page_internal(&conn, limit, before_created_at, before_id)
    }

    fn history_page_internal(
        &self,
        conn: &Connection,
        limit: usize,
        before_created_at: Option<i64>,
        before_id: Option<i64>,
    ) -> Result<Vec<HistoryEntry>> {
        let (sql, params_vec): (String, Vec<rusqlite::types::Value>) = match (before_created_at, before_id) {
            (Some(created), Some(id)) => (
                "SELECT id, capture_uuid, text, raw_text, word_count, duration_ms, delivery_outcome, delivery_warning, created_at
                 FROM history
                 WHERE (created_at < ?1) OR (created_at = ?1 AND id < ?2)
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?3".into(),
                vec![created.into(), id.into(), (limit as i64).into()],
            ),
            _ => (
                "SELECT id, capture_uuid, text, raw_text, word_count, duration_ms, delivery_outcome, delivery_warning, created_at
                 FROM history
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?1".into(),
                vec![(limit as i64).into()],
            ),
        };

        let mut statement = conn.prepare(&sql)?;
        let history = statement
            .query_map(rusqlite::params_from_iter(params_vec), |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    capture_uuid: row.get(1)?,
                    text: row.get(2)?,
                    raw_text: row.get(3)?,
                    word_count: row.get(4)?,
                    duration_ms: row.get(5)?,
                    delivery_outcome: row.get(6)?,
                    delivery_warning: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(history)
    }

    pub fn delete_history_entry(&self, id: i64) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE pending_dictations SET history_id = NULL WHERE history_id = ?1",
            params![id],
        )?;
        let rows = tx.execute("DELETE FROM history WHERE id = ?1", params![id])?;
        if rows == 0 {
            return Err(FlowError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_all_history(&self, delete_pending: bool) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute("UPDATE pending_dictations SET history_id = NULL", [])?;
        tx.execute("DELETE FROM history", [])?;
        if delete_pending {
            tx.execute("DELETE FROM pending_dictations", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn reset_statistics(&self) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE aggregate_stats SET total_words = 0, total_duration_ms = 0 WHERE id = 1",
            [],
        )?;
        tx.execute("DELETE FROM weekly_stats", [])?;
        tx.commit()?;
        Ok(())
    }

    pub fn pending_dictations(&self) -> Result<Vec<PendingDictation>> {
        let conn = self.conn()?;
        self.pending_dictations_internal(&conn)
    }

    fn pending_dictations_internal(&self, conn: &Connection) -> Result<Vec<PendingDictation>> {
        let mut stmt = conn.prepare(
            "SELECT id, capture_uuid, COALESCE(final_text, corrected_text, raw_text, ''),
                    raw_text, corrected_text,
                    CASE WHEN final_text IS NOT NULL THEN 'ready'
                         WHEN raw_text IS NOT NULL THEN 'cleanup'
                         ELSE 'transcription' END,
                    partial, review_reason, no_content, delivery_mode,
                    delivery_outcome, delivery_warning, last_error, error_code,
                    retry_after, history_saved, history_id, created_at
             FROM pending_dictations
             ORDER BY created_at DESC, id DESC",
        )?;
        let pending = stmt
            .query_map([], |row| {
                Ok(PendingDictation {
                    id: row.get(0)?,
                    capture_uuid: row.get(1)?,
                    text: row.get(2)?,
                    raw_text: row.get(3)?,
                    corrected_text: row.get(4)?,
                    stage: row.get(5)?,
                    partial: row.get::<_, i64>(6)? != 0,
                    review_reason: row.get(7)?,
                    no_content: row.get::<_, i64>(8)? != 0,
                    delivery_mode: row.get(9)?,
                    delivery_outcome: row.get(10)?,
                    delivery_warning: row.get(11)?,
                    error: row.get(12)?,
                    error_code: row.get(13)?,
                    retry_after: row.get(14)?,
                    history_saved: row.get::<_, i64>(15)? != 0,
                    history_id: row.get(16)?,
                    created_at: row.get(17)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(pending)
    }

    pub fn insert_pending_recording(
        &self,
        capture_uuid: &str,
        wav: &[u8],
        duration_ms: i64,
        partial: bool,
        review_reason: Option<&str>,
        delivery_mode: &str,
    ) -> Result<i64> {
        let now = Self::now();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO pending_dictations(
                capture_uuid, wav, duration_ms, partial, review_reason,
                delivery_mode, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                capture_uuid,
                wav,
                duration_ms,
                if partial { 1 } else { 0 },
                review_reason,
                delivery_mode,
                now,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn pending_dictation(&self, id: i64) -> Result<PendingDictationRecord> {
        self.conn()?
            .query_row(
                "SELECT id, capture_uuid, wav, raw_text, corrected_text, final_text,
                        duration_ms, partial, review_reason, no_content,
                        delivery_mode, delivery_outcome, delivery_warning,
                        history_saved, history_id
                 FROM pending_dictations WHERE id = ?1",
                [id],
                |row| {
                    Ok(PendingDictationRecord {
                        id: row.get(0)?,
                        capture_uuid: row.get(1)?,
                        wav: row.get(2)?,
                        raw_text: row.get(3)?,
                        corrected_text: row.get(4)?,
                        final_text: row.get(5)?,
                        duration_ms: row.get(6)?,
                        partial: row.get::<_, i64>(7)? != 0,
                        review_reason: row.get(8)?,
                        no_content: row.get::<_, i64>(9)? != 0,
                        delivery_mode: row.get(10)?,
                        delivery_outcome: row.get(11)?,
                        delivery_warning: row.get(12)?,
                        history_saved: row.get::<_, i64>(13)? != 0,
                        history_id: row.get(14)?,
                    })
                },
            )
            .optional()?
            .ok_or(FlowError::NotFound)
    }

    pub fn has_capture_uuid(&self, uuid: &str) -> Result<bool> {
        let conn = self.conn()?;
        let in_pending: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_dictations WHERE capture_uuid = ?1)",
            [uuid],
            |row| row.get(0),
        )?;
        if in_pending {
            return Ok(true);
        }
        let in_history: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM history WHERE capture_uuid = ?1)",
            [uuid],
            |row| row.get(0),
        )?;
        Ok(in_history)
    }

    pub fn save_pending_transcript(
        &self,
        id: i64,
        raw_text: &str,
        corrected_text: &str,
        review_reason: Option<&str>,
    ) -> Result<()> {
        let now = Self::now();
        let rows = self.conn()?.execute(
            "UPDATE pending_dictations
             SET raw_text = ?1, corrected_text = ?2, review_reason = ?3,
                 last_error = NULL, error_code = NULL, updated_at = ?4
             WHERE id = ?5",
            params![raw_text, corrected_text, review_reason, now, id],
        )?;
        if rows == 0 {
            return Err(FlowError::NotFound);
        }
        Ok(())
    }

    pub fn save_pending_final(&self, id: i64, text: &str, no_content: bool) -> Result<()> {
        let now = Self::now();
        let rows = self.conn()?.execute(
            "UPDATE pending_dictations
             SET final_text = ?1, no_content = ?2, last_error = NULL, error_code = NULL, updated_at = ?3
             WHERE id = ?4",
            params![text, if no_content { 1 } else { 0 }, now, id],
        )?;
        if rows == 0 {
            return Err(FlowError::NotFound);
        }
        Ok(())
    }

    pub fn save_pending_error(
        &self,
        id: i64,
        error: &str,
        error_code: Option<&str>,
        retry_after: Option<i64>,
    ) -> Result<()> {
        let now = Self::now();
        let rows = self.conn()?.execute(
            "UPDATE pending_dictations
             SET last_error = ?1, error_code = ?2, retry_after = ?3, updated_at = ?4
             WHERE id = ?5",
            params![error, error_code, retry_after, now, id],
        )?;
        if rows == 0 {
            return Err(FlowError::NotFound);
        }
        Ok(())
    }

    pub fn update_pending_delivery(
        &self,
        id: i64,
        delivery_outcome: &str,
        delivery_warning: Option<&str>,
    ) -> Result<()> {
        let now = Self::now();
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let history_id: Option<i64> = tx
            .query_row(
                "UPDATE pending_dictations
                 SET delivery_outcome = ?1, delivery_warning = ?2, updated_at = ?3
                 WHERE id = ?4
                 RETURNING history_id",
                params![delivery_outcome, delivery_warning, now, id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(FlowError::NotFound)?;

        if let Some(hid) = history_id {
            tx.execute(
                "UPDATE history
                 SET delivery_outcome = ?1, delivery_warning = ?2
                 WHERE id = ?3",
                params![delivery_outcome, delivery_warning, hid],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn accept_pending_transcript(&self, id: i64) -> Result<()> {
        let now = Self::now();
        let rows = self.conn()?.execute(
            "UPDATE pending_dictations
              SET review_reason = NULL, updated_at = ?1
              WHERE id = ?2 AND review_reason = 'suspect_speech' AND history_saved = 0",
            params![now, id],
        )?;
        if rows == 0 {
            return Err(FlowError::NotFound);
        }
        Ok(())
    }

    pub fn save_pending_to_history(&self, id: i64, retention: &str) -> Result<Option<i64>> {
        struct PendingHistoryRow {
            capture_uuid: Option<String>,
            text: Option<String>,
            raw_text: Option<String>,
            duration_ms: i64,
            delivery_outcome: String,
            delivery_warning: Option<String>,
            history_saved: bool,
            existing_history_id: Option<i64>,
        }

        let mut conn = self.conn()?;
        let tx = conn.transaction()?;

        let row: PendingHistoryRow = tx
            .query_row(
                "SELECT capture_uuid, final_text, raw_text, duration_ms, delivery_outcome,
                        delivery_warning, history_saved, history_id
                 FROM pending_dictations WHERE id = ?1",
                [id],
                |r| {
                    Ok(PendingHistoryRow {
                        capture_uuid: r.get(0)?,
                        text: r.get(1)?,
                        raw_text: r.get(2)?,
                        duration_ms: r.get(3)?,
                        delivery_outcome: r.get(4)?,
                        delivery_warning: r.get(5)?,
                        history_saved: r.get::<_, i64>(6)? != 0,
                        existing_history_id: r.get(7)?,
                    })
                },
            )
            .optional()?
            .ok_or(FlowError::NotFound)?;

        if row.history_saved {
            return Ok(row.existing_history_id);
        }

        let text = row.text.ok_or_else(|| {
            FlowError::Message("That recoverable dictation is not ready to save.".into())
        })?;
        let raw_text = row.raw_text.unwrap_or_default();
        let word_count = text.split_whitespace().count() as i64;
        let now = Self::now();

        tx.execute(
            "INSERT INTO history(capture_uuid, text, raw_text, word_count, duration_ms, delivery_outcome, delivery_warning, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![row.capture_uuid, text, raw_text, word_count, row.duration_ms, row.delivery_outcome, row.delivery_warning, now],
        )?;
        let history_id = tx.last_insert_rowid();

        tx.execute(
            "INSERT INTO aggregate_stats(id, total_words, total_duration_ms)
             VALUES(1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                 total_words = total_words + excluded.total_words,
                 total_duration_ms = total_duration_ms + excluded.total_duration_ms",
            params![word_count, row.duration_ms],
        )?;
        tx.execute(
            "INSERT INTO weekly_stats(word_count, duration_ms, created_at) VALUES(?1, ?2, ?3)",
            params![word_count, row.duration_ms, now],
        )?;

        tx.execute(
            "UPDATE pending_dictations SET history_saved = 1, history_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![history_id, now, id],
        )?;

        if let Some(seconds) = retention_seconds(retention) {
            let cutoff = now - seconds;
            tx.execute(
                "UPDATE pending_dictations SET history_id = NULL WHERE history_id IN (SELECT id FROM history WHERE created_at < ?1)",
                [cutoff],
            )?;
            tx.execute("DELETE FROM history WHERE created_at < ?1", [cutoff])?;
        }

        tx.commit()?;
        Ok(Some(history_id))
    }

    pub fn delete_pending(&self, id: i64) -> Result<()> {
        let rows = self
            .conn()?
            .execute("DELETE FROM pending_dictations WHERE id = ?1", [id])?;
        if rows == 0 {
            return Err(FlowError::NotFound);
        }
        Ok(())
    }

    pub fn dictionary(&self) -> Result<Vec<DictionaryEntry>> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT id, value, correction, enabled, conflict_reason, created_at
             FROM dictionary ORDER BY value COLLATE NOCASE",
        )?;
        let entries = statement
            .query_map([], |row| {
                Ok(DictionaryEntry {
                    id: row.get(0)?,
                    value: row.get(1)?,
                    correction: row.get(2)?,
                    enabled: row.get::<_, i64>(3)? != 0,
                    conflict_reason: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn add_dictionary(&self, value: &str, correction: Option<&str>) -> Result<DictionaryEntry> {
        let (value, correction) = validate_dictionary_entry(value, correction)?;
        let now = Self::now();
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;

        let count: i64 = tx.query_row("SELECT COUNT(*) FROM dictionary", [], |row| row.get(0))?;
        if count >= MAX_DICTIONARY_ENTRIES as i64 {
            return Err(FlowError::Message(format!(
                "The dictionary can contain up to {MAX_DICTIONARY_ENTRIES} entries."
            )));
        }

        if let Some(replacement) = &correction {
            if normalized_correction_source_exists(&tx, &value, replacement, None)? {
                return Err(FlowError::Message(
                    "That dictionary entry already exists.".into(),
                ));
            }
        }

        tx.execute(
            "INSERT INTO dictionary(value, correction, enabled, conflict_reason, created_at)
             VALUES(?1, ?2, 1, NULL, ?3)",
            params![value, correction, now],
        )
        .map_err(|error| map_unique_violation(error, "That dictionary entry already exists."))?;
        let new_id = tx.last_insert_rowid();

        tx.commit()?;
        Ok(DictionaryEntry {
            id: new_id,
            value,
            correction,
            enabled: true,
            conflict_reason: None,
            created_at: now,
        })
    }

    pub fn update_dictionary(&self, id: i64, value: &str, correction: Option<&str>) -> Result<()> {
        let (value, correction) = validate_dictionary_entry(value, correction)?;
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;

        if let Some(replacement) = &correction {
            if normalized_correction_source_exists(&tx, &value, replacement, Some(id))? {
                return Err(FlowError::Message(
                    "That dictionary entry already exists.".into(),
                ));
            }
        }

        let rows = tx.execute(
            "UPDATE dictionary SET value = ?1, correction = ?2, enabled = 1, conflict_reason = NULL WHERE id = ?3",
            params![value, correction, id],
        )
        .map_err(|error| map_unique_violation(error, "That dictionary entry already exists."))?;

        if rows == 0 {
            return Err(FlowError::NotFound);
        }

        Self::recompute_dictionary_conflicts(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_dictionary(&self, id: i64) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let rows = tx.execute("DELETE FROM dictionary WHERE id = ?1", [id])?;
        if rows == 0 {
            return Err(FlowError::NotFound);
        }
        Self::recompute_dictionary_conflicts(&tx)?;
        tx.commit()?;
        Ok(())
    }

    fn recompute_dictionary_conflicts(conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare(
            "SELECT id, value, correction FROM dictionary WHERE correction IS NOT NULL AND trim(correction) != ''",
        )?;
        let entries: Vec<(i64, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter_map(std::result::Result::ok)
            .collect();
        drop(stmt);

        // Conflict classification is shared with the correction engine: only
        // distinct replacements for the same normalized source conflict;
        // identical replacements are the same rule and stay enabled.
        let rules: Vec<(String, String)> = entries
            .iter()
            .map(|(_, value, correction)| (value.clone(), correction.clone()))
            .collect();
        let conflicts = text::conflicting_correction_sources(&rules);

        for (id, value, _) in entries {
            let normalized = text::normalize_key(&value);
            if normalized.is_empty() {
                continue;
            }
            if conflicts.contains(&normalized) {
                conn.execute(
                    "UPDATE dictionary SET enabled = 0, conflict_reason = 'normalized_source_conflict' WHERE id = ?1",
                    [id],
                )?;
            } else {
                conn.execute(
                    "UPDATE dictionary SET enabled = 1, conflict_reason = NULL WHERE id = ?1",
                    [id],
                )?;
            }
        }
        Ok(())
    }

    pub fn snippets(&self) -> Result<Vec<Snippet>> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT id, trigger, content, enabled, conflict_reason, created_at
             FROM snippets ORDER BY created_at",
        )?;
        let snippets = statement
            .query_map([], |row| {
                Ok(Snippet {
                    id: row.get(0)?,
                    trigger: row.get(1)?,
                    content: row.get(2)?,
                    enabled: row.get::<_, i64>(3)? != 0,
                    conflict_reason: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(snippets)
    }

    pub fn add_snippet(&self, trigger: &str, content: &str) -> Result<Snippet> {
        let (trigger, content) = validate_snippet(trigger, content)?;
        let now = Self::now();
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;

        let count: i64 = tx.query_row("SELECT COUNT(*) FROM snippets", [], |row| row.get(0))?;
        if count >= MAX_SNIPPETS as i64 {
            return Err(FlowError::Message(format!(
                "You can save up to {MAX_SNIPPETS} snippets."
            )));
        }

        if normalized_trigger_exists(&tx, &trigger, None)? {
            return Err(FlowError::Message(
                "That snippet trigger already exists.".into(),
            ));
        }

        tx.execute(
            "INSERT INTO snippets(trigger, content, enabled, conflict_reason, created_at)
             VALUES(?1, ?2, 1, NULL, ?3)",
            params![trigger, content, now],
        )
        .map_err(|error| map_unique_violation(error, "That snippet trigger already exists."))?;
        let new_id = tx.last_insert_rowid();

        tx.commit()?;
        Ok(Snippet {
            id: new_id,
            trigger,
            content,
            enabled: true,
            conflict_reason: None,
            created_at: now,
        })
    }

    pub fn update_snippet(&self, id: i64, trigger: &str, content: &str) -> Result<()> {
        let (trigger, content) = validate_snippet(trigger, content)?;
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;

        if normalized_trigger_exists(&tx, &trigger, Some(id))? {
            return Err(FlowError::Message(
                "That snippet trigger already exists.".into(),
            ));
        }

        let rows = tx.execute(
            "UPDATE snippets SET trigger = ?1, content = ?2, enabled = 1, conflict_reason = NULL WHERE id = ?3",
            params![trigger, content, id],
        )?;

        if rows == 0 {
            return Err(FlowError::NotFound);
        }

        Self::recompute_snippet_conflicts(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_snippet(&self, id: i64) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let rows = tx.execute("DELETE FROM snippets WHERE id = ?1", [id])?;
        if rows == 0 {
            return Err(FlowError::NotFound);
        }
        Self::recompute_snippet_conflicts(&tx)?;
        tx.commit()?;
        Ok(())
    }

    fn recompute_snippet_conflicts(conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare("SELECT id, trigger FROM snippets")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(std::result::Result::ok)
            .collect();
        drop(stmt);

        let mut groups: HashMap<String, Vec<i64>> = HashMap::new();
        for (id, trig) in rows {
            if let Some(norm) = text::normalize_snippet_trigger(&trig) {
                groups.entry(norm).or_default().push(id);
            }
        }

        for (_, ids) in groups {
            if ids.len() == 1 {
                conn.execute(
                    "UPDATE snippets SET enabled = 1, conflict_reason = NULL WHERE id = ?1",
                    [ids[0]],
                )?;
            } else {
                for id in ids {
                    conn.execute(
                        "UPDATE snippets SET enabled = 0, conflict_reason = 'normalized_trigger_conflict' WHERE id = ?1",
                        [id],
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn app_config(&self) -> AppConfig {
        AppConfig::default()
    }
}

fn validate_dictionary_entry(
    value: &str,
    correction: Option<&str>,
) -> Result<(String, Option<String>)> {
    let value = value.trim();
    if value.is_empty() {
        return Err(FlowError::Message("Enter a word or name.".into()));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(FlowError::Message(
            "Control characters are not allowed.".into(),
        ));
    }
    if value.chars().count() > MAX_DICTIONARY_SOURCE_CHARS {
        return Err(FlowError::Message(format!(
            "Dictionary entries can contain up to {MAX_DICTIONARY_SOURCE_CHARS} characters."
        )));
    }
    if correction.is_some() && text::normalize_correction_source(value).is_empty() {
        return Err(FlowError::Message("Enter a word or name.".into()));
    }
    let correction = correction.map(str::trim);
    if correction.is_some_and(str::is_empty) {
        return Err(FlowError::Message("Enter the correct spelling.".into()));
    }
    if let Some(corr) = correction {
        if corr.chars().any(|c| c.is_control()) {
            return Err(FlowError::Message(
                "Control characters are not allowed.".into(),
            ));
        }
        if corr.chars().count() > MAX_DICTIONARY_CORRECTION_CHARS {
            return Err(FlowError::Message(format!(
                "Dictionary corrections can contain up to {MAX_DICTIONARY_CORRECTION_CHARS} characters."
            )));
        }
        if text::normalize_key(corr) == text::normalize_key(value) && corr == value {
            return Err(FlowError::Message(
                "The misspelling and correction must be different.".into(),
            ));
        }
    }
    Ok((value.into(), correction.map(str::to_owned)))
}

fn validate_snippet(trigger: &str, content: &str) -> Result<(String, String)> {
    let trigger = trigger.trim();
    if trigger.is_empty() || content.trim().is_empty() {
        return Err(FlowError::Message(
            "A snippet needs both a trigger and content.".into(),
        ));
    }
    if trigger.chars().any(|c| c.is_control()) {
        return Err(FlowError::Message(
            "Control characters are not allowed in triggers.".into(),
        ));
    }
    if trigger.chars().count() > MAX_SNIPPET_TRIGGER_CHARS {
        return Err(FlowError::Message(format!(
            "Snippet triggers can contain up to {MAX_SNIPPET_TRIGGER_CHARS} characters."
        )));
    }
    if text::normalize_snippet_trigger(trigger).is_none() {
        return Err(FlowError::Message(
            "Snippet trigger must contain at least one letter or number.".into(),
        ));
    }

    if content
        .chars()
        .any(|c| c == '\0' || (c.is_control() && c != '\t' && c != '\r' && c != '\n'))
    {
        return Err(FlowError::Message(
            "Disallowed control characters in content.".into(),
        ));
    }
    if content.chars().count() > MAX_SNIPPET_CONTENT_CHARS {
        return Err(FlowError::Message(format!(
            "Snippet content can contain up to {MAX_SNIPPET_CONTENT_CHARS} characters."
        )));
    }

    Ok((trigger.into(), content.into()))
}

fn retention_seconds(retention: &str) -> Option<i64> {
    match retention {
        "24 hours" => Some(86_400),
        "7 days" => Some(7 * 86_400),
        "30 days" => Some(30 * 86_400),
        _ => None,
    }
}

fn normalized_trigger_exists(
    conn: &Connection,
    trigger: &str,
    excluded_id: Option<i64>,
) -> Result<bool> {
    let Some(normalized) = text::normalize_snippet_trigger(trigger) else {
        return Ok(false);
    };
    let mut statement = conn.prepare("SELECT id, trigger FROM snippets")?;
    let triggers = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(triggers.into_iter().any(|(id, existing)| {
        Some(id) != excluded_id
            && text::normalize_snippet_trigger(&existing) == Some(normalized.clone())
    }))
}

fn normalized_correction_source_exists(
    conn: &Connection,
    value: &str,
    replacement: &str,
    excluded_id: Option<i64>,
) -> Result<bool> {
    let normalized = text::normalize_correction_source(value);
    let mut statement = conn.prepare(
        "SELECT id, value, correction FROM dictionary WHERE correction IS NOT NULL AND trim(correction) != ''",
    )?;
    let entries = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // A source is only taken when another entry shares its normalized source
    // with a different replacement; an identical replacement is the same rule.
    Ok(entries
        .into_iter()
        .any(|(id, existing, existing_replacement)| {
            Some(id) != excluded_id
                && text::normalize_correction_source(&existing) == normalized
                && existing_replacement != replacement
        }))
}

fn map_unique_violation(error: rusqlite::Error, message: &str) -> FlowError {
    if let rusqlite::Error::SqliteFailure(err, _) = &error {
        if err.extended_code == 2067 || err.extended_code == 1555 {
            return FlowError::Message(message.into());
        }
    }
    FlowError::Database(error)
}

#[cfg(windows)]
pub fn get_available_disk_space(path: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let path_wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_bytes = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(
            windows::core::PCWSTR(path_wide.as_ptr()),
            Some(&mut free_bytes),
            None,
            None,
        )
        .map_err(|e| FlowError::Windows(format!("Failed to query available disk space: {e}")))?;
    }
    Ok(free_bytes)
}

#[cfg(not(windows))]
pub fn get_available_disk_space(_path: &Path) -> Result<u64> {
    Ok(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::models::SettingsData;
    use rusqlite::Connection;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static DATABASE_ID: AtomicU64 = AtomicU64::new(0);

    fn database_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "flow-{test_name}-{}-{}.sqlite3",
            std::process::id(),
            DATABASE_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn opening_an_existing_database_adds_nullable_corrections_and_migrates_to_v2() {
        let path = database_path("dictionary-migration");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "
                CREATE TABLE dictionary (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    value TEXT NOT NULL COLLATE NOCASE UNIQUE,
                    created_at INTEGER NOT NULL
                );
                INSERT INTO dictionary(value, created_at) VALUES('Flow', 1);
                ",
            )
            .unwrap();
        drop(legacy);

        let database = Database::open(&path).unwrap();
        let entries = database.dictionary().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value, "Flow");
        assert_eq!(entries[0].correction, None);
        assert!(entries[0].enabled);

        let user_ver: i32 = database
            .conn()
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(user_ver, 2);

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn opening_an_existing_database_migrates_dictionary_conflicts_using_replacements() {
        let path = database_path("dictionary-migration-conflicts");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "
                CREATE TABLE dictionary (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    value TEXT NOT NULL COLLATE NOCASE UNIQUE,
                    correction TEXT,
                    created_at INTEGER NOT NULL
                );
                INSERT INTO dictionary(value, correction, created_at) VALUES('four word', 'Forward', 1);
                INSERT INTO dictionary(value, correction, created_at) VALUES('four   word', 'Forward', 2);
                INSERT INTO dictionary(value, correction, created_at) VALUES('teh', 'the', 3);
                INSERT INTO dictionary(value, correction, created_at) VALUES('teh  ', 'that', 4);
                ",
            )
            .unwrap();
        drop(legacy);

        let database = Database::open(&path).unwrap();
        let entries = database.dictionary().unwrap();

        let forward_entries: Vec<_> = entries
            .iter()
            .filter(|e| e.correction.as_deref() == Some("Forward"))
            .collect();
        assert_eq!(forward_entries.len(), 2);
        assert!(forward_entries.iter().all(|e| e.enabled));
        assert!(forward_entries.iter().all(|e| e.conflict_reason.is_none()));

        let teh_the = entries.iter().find(|e| e.value == "teh").unwrap();
        assert!(!teh_the.enabled);
        assert_eq!(
            teh_the.conflict_reason.as_deref(),
            Some("normalized_source_conflict")
        );

        let teh_that = entries.iter().find(|e| e.value == "teh  ").unwrap();
        assert!(!teh_that.enabled);
        assert_eq!(
            teh_that.conflict_reason.as_deref(),
            Some("normalized_source_conflict")
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn dictionary_entries_can_convert_between_words_and_corrections() {
        let path = database_path("dictionary-corrections");
        let database = Database::open(&path).unwrap();

        let entry = database
            .add_dictionary("  btw ", Some(" by the way  "))
            .unwrap();
        assert_eq!(entry.value, "btw");
        assert_eq!(entry.correction.as_deref(), Some("by the way"));

        database.update_dictionary(entry.id, "Flow", None).unwrap();
        let entries = database.dictionary().unwrap();
        assert_eq!(entries[0].value, "Flow");
        assert_eq!(entries[0].correction, None);

        database
            .update_dictionary(entry.id, "same", Some("SAME"))
            .unwrap();
        let entries = database.dictionary().unwrap();
        assert_eq!(entries[0].correction.as_deref(), Some("SAME"));

        let error = database
            .update_dictionary(entry.id, "same", Some("same"))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The misspelling and correction must be different."
        );
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn dictionary_rejects_duplicate_normalized_correction_sources() {
        let path = database_path("normalized-dictionary-corrections");
        let database = Database::open(&path).unwrap();

        database
            .add_dictionary("four word", Some("Forward"))
            .unwrap();
        let regular = database.add_dictionary("foo.", None).unwrap();

        let empty_source_error = database.add_dictionary("...", Some("Forward")).unwrap_err();
        assert_eq!(empty_source_error.to_string(), "Enter a word or name.");

        let add_error = database
            .add_dictionary("four   word.", Some("Foreword"))
            .unwrap_err();
        assert_eq!(
            add_error.to_string(),
            "That dictionary entry already exists."
        );

        database
            .add_dictionary("foo", Some("food"))
            .expect("regular entries do not create correction rules");
        let update_error = database
            .update_dictionary(regular.id, "FOO.", Some("fool"))
            .unwrap_err();
        assert_eq!(
            update_error.to_string(),
            "That dictionary entry already exists."
        );

        // A source whose replacement is identical to the existing entry is the
        // same rule: it is allowed, and recomputation keeps both entries
        // enabled instead of flagging a conflict.
        let same_rule = database
            .add_dictionary("four   word", Some("Forward"))
            .expect("identical replacements for one source are allowed");
        database
            .update_dictionary(same_rule.id, "four   word", Some("Forward"))
            .expect("recompute keeps identical-replacement entries enabled");
        let entries = database.dictionary().unwrap();
        let forward_rules: Vec<_> = entries
            .iter()
            .filter(|e| e.correction.as_deref() == Some("Forward"))
            .collect();
        assert_eq!(forward_rules.len(), 2);
        assert!(forward_rules.iter().all(|e| e.enabled));
        assert!(forward_rules.iter().all(|e| e.conflict_reason.is_none()));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pending_dictation_keeps_recoverable_stages_and_commits_once() {
        let path = database_path("pending-dictation");
        let database = Database::open(&path).unwrap();
        let id = database
            .insert_pending_recording("uuid-123", b"wav", 1_500, false, None, "automatic")
            .unwrap();
        let not_ready = database.save_pending_to_history(id, "30 days").unwrap_err();
        assert_eq!(
            not_ready.to_string(),
            "That recoverable dictation is not ready to save."
        );
        let recording = database.pending_dictation(id).unwrap();
        assert_eq!(recording.wav.as_deref(), Some(b"wav".as_slice()));

        database
            .save_pending_transcript(id, "raw words", "raw words", None)
            .unwrap();
        database
            .save_pending_final(id, "Final words", false)
            .unwrap();
        database.save_pending_to_history(id, "30 days").unwrap();
        database.save_pending_to_history(id, "30 days").unwrap();
        let dashboard = database.dashboard().unwrap();
        assert_eq!(dashboard.history.len(), 1);
        assert_eq!(dashboard.pending.len(), 1);
        assert_eq!(dashboard.pending[0].text, "Final words");

        database.delete_pending(id).unwrap();
        assert!(database.dashboard().unwrap().pending.is_empty());
        let missing = database.save_pending_to_history(id, "30 days").unwrap_err();
        assert!(matches!(missing, crate::error::FlowError::NotFound));
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn weekly_stats_outlive_short_content_retention() {
        let path = database_path("weekly-retention");
        let database = Database::open(&path).unwrap();
        let id = database
            .insert_pending_recording("uuid-w1", b"wav", 1_000, false, None, "automatic")
            .unwrap();
        database
            .save_pending_transcript(id, "one two", "one two", None)
            .unwrap();
        database.save_pending_final(id, "one two", false).unwrap();
        database.save_pending_to_history(id, "30 days").unwrap();

        let two_days_ago = Database::now() - 2 * 86_400;
        database
            .conn()
            .unwrap()
            .execute("UPDATE history SET created_at = ?1", [two_days_ago])
            .unwrap();
        database
            .conn()
            .unwrap()
            .execute("UPDATE weekly_stats SET created_at = ?1", [two_days_ago])
            .unwrap();
        let settings = SettingsData {
            history_retention: "24 hours".into(),
            ..SettingsData::default()
        };
        database.save_settings(&settings).unwrap();

        let dashboard = database.dashboard().unwrap();
        assert!(dashboard.history.is_empty());
        assert_eq!(dashboard.time_dictated_ms, 1_000);
        assert_eq!(dashboard.estimated_saved_ms, 2_000);
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_probe_22_history_idempotence() {
        let path = database_path("probe-22-idempotence");
        let database = Database::open(&path).unwrap();
        let id = database
            .insert_pending_recording("uuid-p22", b"wav", 1_000, false, None, "automatic")
            .unwrap();
        database
            .save_pending_transcript(id, "two words", "two words", None)
            .unwrap();
        database.save_pending_final(id, "two words", false).unwrap();

        // First save creates history and stats
        let history_id = database
            .save_pending_to_history(id, "30 days")
            .unwrap()
            .unwrap();
        let dash1 = database.dashboard().unwrap();
        assert_eq!(dash1.total_words_dictated, 2);
        assert_eq!(dash1.history.len(), 1);

        // Explicitly delete history entry
        database.delete_history_entry(history_id).unwrap();
        let dash2 = database.dashboard().unwrap();
        assert_eq!(dash2.history.len(), 0);

        // Invoking save_pending_to_history again: history_saved is authoritative!
        // It must NOT recreate history or increment total words
        let second_result = database.save_pending_to_history(id, "30 days").unwrap();
        assert!(second_result.is_none());

        let dash3 = database.dashboard().unwrap();
        assert_eq!(dash3.history.len(), 0);
        assert_eq!(dash3.total_words_dictated, 2);

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_probe_23_update_delivery_updates_history() {
        let path = database_path("probe-23-delivery");
        let database = Database::open(&path).unwrap();
        let id = database
            .insert_pending_recording("uuid-p23", b"wav", 1_000, false, None, "automatic")
            .unwrap();
        database
            .save_pending_transcript(id, "hello", "hello", None)
            .unwrap();
        database.save_pending_final(id, "hello", false).unwrap();

        let _ = database.save_pending_to_history(id, "30 days").unwrap();

        // Update pending delivery to copied
        database
            .update_pending_delivery(id, "copied", Some("warning"))
            .unwrap();

        // Verify history row now reflects copied
        let dash = database.dashboard().unwrap();
        assert_eq!(dash.history.len(), 1);
        assert_eq!(dash.history[0].delivery_outcome, "copied");
        assert_eq!(dash.history[0].delivery_warning.as_deref(), Some("warning"));

        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
