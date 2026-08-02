use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension};

use crate::{
    error::{FlowError, Result},
    models::{
        DashboardData, DictionaryEntry, HistoryEntry, PendingDictation, SettingsData, Snippet,
    },
};

const MAX_DICTIONARY_ENTRIES: i64 = 1_000;
const MAX_DICTIONARY_VALUE_CHARS: usize = 100;
const MAX_DICTIONARY_CORRECTION_CHARS: usize = 200;

pub struct PendingDictationRecord {
    pub id: i64,
    pub wav: Option<Vec<u8>>,
    pub raw_text: Option<String>,
    pub final_text: Option<String>,
    pub duration_ms: i64,
}

pub struct Database {
    connection: Mutex<Connection>,
}

fn map_unique_violation(error: rusqlite::Error, message: &str) -> FlowError {
    match error {
        rusqlite::Error::SqliteFailure(ref failure, _)
            if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
        {
            FlowError::Message(message.into())
        }
        other => FlowError::Database(other),
    }
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                FlowError::Message(format!("Could not create Flow data folder: {error}"))
            })?;
        }
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
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
            CREATE INDEX IF NOT EXISTS weekly_stats_created_at
                ON weekly_stats(created_at);
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
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| FlowError::Message("The local database is unavailable.".into()))
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
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
        })
    }

    pub fn save_settings(&self, settings: &SettingsData) -> Result<()> {
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
        if let Some(seconds) = retention_seconds(&settings.history_retention) {
            transaction.execute(
                "DELETE FROM history WHERE created_at < ?1",
                [Self::now() - seconds],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn prune_history(&self, retention: &str) -> Result<()> {
        if let Some(seconds) = retention_seconds(retention) {
            self.conn()?.execute(
                "DELETE FROM history WHERE created_at < ?1",
                [Self::now() - seconds],
            )?;
        }
        Ok(())
    }

    pub fn dashboard(&self) -> Result<DashboardData> {
        let retention = self
            .setting("history_retention")?
            .unwrap_or_else(|| SettingsData::default().history_retention);
        self.prune_history(&retention)?;
        let week_ago = Self::now() - 7 * 86_400;
        let conn = self.conn()?;
        conn.execute("DELETE FROM weekly_stats WHERE created_at < ?1", [week_ago])?;
        let (total_words, total_duration): (i64, i64) = conn.query_row(
            "SELECT total_words, total_duration_ms
             FROM aggregate_stats WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (weekly_words, weekly_duration): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(word_count), 0), COALESCE(SUM(duration_ms), 0)
             FROM weekly_stats",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let mut statement = conn.prepare(
            "SELECT id, text, raw_text, word_count, duration_ms, created_at
             FROM history ORDER BY created_at DESC LIMIT 100",
        )?;
        let history = statement
            .query_map([], |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    raw_text: row.get(2)?,
                    word_count: row.get(3)?,
                    duration_ms: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut pending_statement = conn.prepare(
            "SELECT id, COALESCE(final_text, raw_text, ''),
                    CASE WHEN final_text IS NOT NULL THEN 'ready'
                         WHEN raw_text IS NOT NULL THEN 'cleanup'
                         ELSE 'transcription' END,
                    last_error, created_at
             FROM pending_dictations ORDER BY created_at DESC",
        )?;
        let pending = pending_statement
            .query_map([], |row| {
                Ok(PendingDictation {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    stage: row.get(2)?,
                    error: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let average_words_per_minute = if total_duration > 0 {
            total_words
                .saturating_mul(60_000)
                .saturating_add(total_duration / 2)
                / total_duration
        } else {
            0
        };
        // Comfortable typing is ~40 wpm, or 1.5 seconds per word.
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

    pub fn insert_history(&self, text: &str, raw_text: &str, duration_ms: i64) -> Result<()> {
        let word_count = text.split_whitespace().count() as i64;
        let mut conn = self.conn()?;
        let transaction = conn.transaction()?;
        transaction.execute(
            "INSERT INTO history(text, raw_text, word_count, duration_ms, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![text, raw_text, word_count, duration_ms, Self::now()],
        )?;
        transaction.execute(
            "INSERT INTO aggregate_stats(id, total_words, total_duration_ms)
             VALUES(1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                 total_words = total_words + excluded.total_words,
                 total_duration_ms = total_duration_ms + excluded.total_duration_ms",
            params![word_count, duration_ms],
        )?;
        transaction.execute(
            "INSERT INTO weekly_stats(word_count, duration_ms, created_at) VALUES(?1, ?2, ?3)",
            params![word_count, duration_ms, Self::now()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn insert_pending_recording(&self, wav: &[u8], duration_ms: i64) -> Result<i64> {
        let now = Self::now();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO pending_dictations(wav, duration_ms, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?3)",
            params![wav, duration_ms, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn pending_dictation(&self, id: i64) -> Result<PendingDictationRecord> {
        self.conn()?
            .query_row(
                "SELECT id, wav, raw_text, final_text, duration_ms
                 FROM pending_dictations WHERE id = ?1",
                [id],
                |row| {
                    Ok(PendingDictationRecord {
                        id: row.get(0)?,
                        wav: row.get(1)?,
                        raw_text: row.get(2)?,
                        final_text: row.get(3)?,
                        duration_ms: row.get(4)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| {
                FlowError::Message("That recoverable dictation no longer exists.".into())
            })
    }

    pub fn save_pending_transcript(&self, id: i64, transcript: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE pending_dictations
             SET wav = NULL, raw_text = ?1, last_error = NULL, updated_at = ?2
             WHERE id = ?3",
            params![transcript, Self::now(), id],
        )?;
        Ok(())
    }

    pub fn save_pending_final(&self, id: i64, text: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE pending_dictations
             SET final_text = ?1, last_error = NULL, updated_at = ?2 WHERE id = ?3",
            params![text, Self::now(), id],
        )?;
        Ok(())
    }

    pub fn save_pending_error(&self, id: i64, error: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE pending_dictations SET last_error = ?1, updated_at = ?2 WHERE id = ?3",
            params![error, Self::now(), id],
        )?;
        Ok(())
    }

    pub fn save_pending_to_history(&self, id: i64, retention: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let transaction = conn.transaction()?;
        let (text, raw_text, duration_ms, history_saved): (
            Option<String>,
            Option<String>,
            i64,
            bool,
        ) = transaction
            .query_row(
                "SELECT final_text, raw_text, duration_ms, history_saved
             FROM pending_dictations WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?
            .ok_or_else(|| {
                FlowError::Message("That recoverable dictation no longer exists.".into())
            })?;
        if history_saved {
            return Ok(());
        }
        let text = text.ok_or_else(|| {
            FlowError::Message("That recoverable dictation is not ready to save.".into())
        })?;
        let raw_text = raw_text.unwrap_or_default();
        let word_count = text.split_whitespace().count() as i64;
        let now = Self::now();
        transaction.execute(
            "INSERT INTO history(text, raw_text, word_count, duration_ms, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![text, raw_text, word_count, duration_ms, now],
        )?;
        transaction.execute(
            "INSERT INTO aggregate_stats(id, total_words, total_duration_ms)
             VALUES(1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET total_words = total_words + excluded.total_words,
                 total_duration_ms = total_duration_ms + excluded.total_duration_ms",
            params![word_count, duration_ms],
        )?;
        transaction.execute(
            "INSERT INTO weekly_stats(word_count, duration_ms, created_at) VALUES(?1, ?2, ?3)",
            params![word_count, duration_ms, now],
        )?;
        transaction.execute(
            "UPDATE pending_dictations SET history_saved = 1, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        if let Some(seconds) = retention_seconds(retention) {
            transaction.execute("DELETE FROM history WHERE created_at < ?1", [now - seconds])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_pending(&self, id: i64) -> Result<()> {
        self.conn()?
            .execute("DELETE FROM pending_dictations WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn dictionary(&self) -> Result<Vec<DictionaryEntry>> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT id, value, correction, created_at
             FROM dictionary ORDER BY value COLLATE NOCASE",
        )?;
        let entries = statement
            .query_map([], |row| {
                Ok(DictionaryEntry {
                    id: row.get(0)?,
                    value: row.get(1)?,
                    correction: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn add_dictionary(&self, value: &str, correction: Option<&str>) -> Result<DictionaryEntry> {
        let (value, correction) = validate_dictionary_entry(value, correction)?;
        let now = Self::now();
        let conn = self.conn()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM dictionary", [], |row| row.get(0))?;
        if count >= MAX_DICTIONARY_ENTRIES {
            return Err(FlowError::Message(format!(
                "The dictionary can contain up to {MAX_DICTIONARY_ENTRIES} entries."
            )));
        }
        if correction.is_some() && normalized_correction_source_exists(&conn, &value, None)? {
            return Err(FlowError::Message(
                "That dictionary entry already exists.".into(),
            ));
        }
        conn.execute(
            "INSERT INTO dictionary(value, correction, created_at) VALUES(?1, ?2, ?3)",
            params![value, correction, now],
        )
        .map_err(|error| map_unique_violation(error, "That dictionary entry already exists."))?;
        Ok(DictionaryEntry {
            id: conn.last_insert_rowid(),
            value,
            correction,
            created_at: now,
        })
    }

    pub fn update_dictionary(&self, id: i64, value: &str, correction: Option<&str>) -> Result<()> {
        let (value, correction) = validate_dictionary_entry(value, correction)?;
        let conn = self.conn()?;
        if correction.is_some() && normalized_correction_source_exists(&conn, &value, Some(id))? {
            return Err(FlowError::Message(
                "That dictionary entry already exists.".into(),
            ));
        }
        conn.execute(
            "UPDATE dictionary SET value = ?1, correction = ?2 WHERE id = ?3",
            params![value, correction, id],
        )
        .map_err(|error| map_unique_violation(error, "That dictionary entry already exists."))?;
        Ok(())
    }

    pub fn delete_dictionary(&self, id: i64) -> Result<()> {
        self.conn()?
            .execute("DELETE FROM dictionary WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn snippets(&self) -> Result<Vec<Snippet>> {
        let conn = self.conn()?;
        let mut statement = conn
            .prepare("SELECT id, trigger, content, created_at FROM snippets ORDER BY created_at")?;
        let snippets = statement
            .query_map([], |row| {
                Ok(Snippet {
                    id: row.get(0)?,
                    trigger: row.get(1)?,
                    content: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(snippets)
    }

    pub fn add_snippet(&self, trigger: &str, content: &str) -> Result<Snippet> {
        let trigger = trigger.trim();
        if crate::workflow::normalize_utterance(trigger).is_empty() || content.trim().is_empty() {
            return Err(FlowError::Message(
                "A snippet needs both a trigger and content.".into(),
            ));
        }
        let now = Self::now();
        let conn = self.conn()?;
        if normalized_trigger_exists(&conn, trigger, None)? {
            return Err(FlowError::Message(
                "That snippet trigger already exists.".into(),
            ));
        }
        conn.execute(
            "INSERT INTO snippets(trigger, content, created_at) VALUES(?1, ?2, ?3)",
            params![trigger, content, now],
        )
        .map_err(|error| map_unique_violation(error, "That snippet trigger already exists."))?;
        Ok(Snippet {
            id: conn.last_insert_rowid(),
            trigger: trigger.into(),
            content: content.into(),
            created_at: now,
        })
    }

    pub fn update_snippet(&self, id: i64, trigger: &str, content: &str) -> Result<()> {
        let trigger = trigger.trim();
        if crate::workflow::normalize_utterance(trigger).is_empty() || content.trim().is_empty() {
            return Err(FlowError::Message(
                "A snippet needs both a trigger and content.".into(),
            ));
        }
        let conn = self.conn()?;
        if normalized_trigger_exists(&conn, trigger, Some(id))? {
            return Err(FlowError::Message(
                "That snippet trigger already exists.".into(),
            ));
        }
        conn.execute(
            "UPDATE snippets SET trigger = ?1, content = ?2 WHERE id = ?3",
            params![trigger, content, id],
        )?;
        Ok(())
    }

    pub fn delete_snippet(&self, id: i64) -> Result<()> {
        self.conn()?
            .execute("DELETE FROM snippets WHERE id = ?1", [id])?;
        Ok(())
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
    if value.chars().count() > MAX_DICTIONARY_VALUE_CHARS {
        return Err(FlowError::Message(format!(
            "Dictionary entries can contain up to {MAX_DICTIONARY_VALUE_CHARS} characters."
        )));
    }
    if correction.is_some() && crate::workflow::normalize_correction_source(value).is_empty() {
        return Err(FlowError::Message("Enter a word or name.".into()));
    }
    let correction = correction.map(str::trim);
    if correction.is_some_and(str::is_empty) {
        return Err(FlowError::Message("Enter the correct spelling.".into()));
    }
    if correction.is_some_and(|value| value.chars().count() > MAX_DICTIONARY_CORRECTION_CHARS) {
        return Err(FlowError::Message(format!(
            "Dictionary corrections can contain up to {MAX_DICTIONARY_CORRECTION_CHARS} characters."
        )));
    }
    if correction.is_some_and(|correction| correction == value) {
        return Err(FlowError::Message(
            "The misspelling and correction must be different.".into(),
        ));
    }
    Ok((value.into(), correction.map(str::to_owned)))
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
    let normalized = crate::workflow::normalize_utterance(trigger);
    let mut statement = conn.prepare("SELECT id, trigger FROM snippets")?;
    let triggers = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(triggers.into_iter().any(|(id, existing)| {
        Some(id) != excluded_id && crate::workflow::normalize_utterance(&existing) == normalized
    }))
}

fn normalized_correction_source_exists(
    conn: &Connection,
    value: &str,
    excluded_id: Option<i64>,
) -> Result<bool> {
    let normalized = crate::workflow::normalize_correction_source(value);
    let mut statement =
        conn.prepare("SELECT id, value FROM dictionary WHERE correction IS NOT NULL")?;
    let entries = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(entries.into_iter().any(|(id, existing)| {
        Some(id) != excluded_id
            && crate::workflow::normalize_correction_source(&existing) == normalized
    }))
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use rusqlite::Connection;

    use crate::models::SettingsData;

    use super::Database;

    static DATABASE_ID: AtomicU64 = AtomicU64::new(0);

    fn database_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "flow-{test_name}-{}-{}.sqlite3",
            std::process::id(),
            DATABASE_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn opening_an_existing_database_adds_nullable_corrections() {
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

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pending_dictation_keeps_recoverable_stages_and_commits_once() {
        let path = database_path("pending-dictation");
        let database = Database::open(&path).unwrap();
        let id = database.insert_pending_recording(b"wav", 1_500).unwrap();
        let not_ready = database.save_pending_to_history(id, "30 days").unwrap_err();
        assert_eq!(
            not_ready.to_string(),
            "That recoverable dictation is not ready to save."
        );
        let recording = database.pending_dictation(id).unwrap();
        assert_eq!(recording.wav.as_deref(), Some(b"wav".as_slice()));

        database.save_pending_transcript(id, "raw words").unwrap();
        let transcript = database.pending_dictation(id).unwrap();
        assert!(transcript.wav.is_none());
        assert_eq!(transcript.raw_text.as_deref(), Some("raw words"));

        database.save_pending_final(id, "Final words").unwrap();
        database.save_pending_to_history(id, "30 days").unwrap();
        database.save_pending_to_history(id, "30 days").unwrap();
        let dashboard = database.dashboard().unwrap();
        assert_eq!(dashboard.history.len(), 1);
        assert_eq!(dashboard.pending.len(), 1);
        assert_eq!(dashboard.pending[0].text, "Final words");

        database.delete_pending(id).unwrap();
        assert!(database.dashboard().unwrap().pending.is_empty());
        let missing = database.save_pending_to_history(id, "30 days").unwrap_err();
        assert_eq!(
            missing.to_string(),
            "That recoverable dictation no longer exists."
        );
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn weekly_stats_outlive_short_content_retention() {
        let path = database_path("weekly-retention");
        let database = Database::open(&path).unwrap();
        database
            .insert_history("one two", "one two", 1_000)
            .unwrap();
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
}
