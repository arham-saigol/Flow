use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension};

use crate::{
    error::{FlowError, Result},
    models::{DashboardData, DictionaryEntry, HistoryEntry, SettingsData, Snippet},
};

pub struct Database {
    connection: Mutex<Connection>,
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
            CREATE TABLE IF NOT EXISTS dictionary (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                value TEXT NOT NULL COLLATE NOCASE UNIQUE,
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

    fn put_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn()?.execute(
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
            automatic_language: self
                .setting("automatic_language")?
                .map(|value| value == "true")
                .unwrap_or(defaults.automatic_language),
        })
    }

    pub fn save_settings(&self, settings: &SettingsData) -> Result<()> {
        self.put_setting("microphone_id", &settings.microphone_id)?;
        self.put_setting("microphone_name", &settings.microphone_name)?;
        self.put_setting("keybind", &settings.keybind)?;
        self.put_setting(
            "launch_at_startup",
            if settings.launch_at_startup {
                "true"
            } else {
                "false"
            },
        )?;
        self.put_setting("history_retention", &settings.history_retention)?;
        self.put_setting(
            "automatic_language",
            if settings.automatic_language {
                "true"
            } else {
                "false"
            },
        )?;
        self.prune_history(&settings.history_retention)
    }

    pub fn prune_history(&self, retention: &str) -> Result<()> {
        let seconds = match retention {
            "24 hours" => Some(86_400),
            "7 days" => Some(7 * 86_400),
            "30 days" => Some(30 * 86_400),
            _ => None,
        };
        if let Some(seconds) = seconds {
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
        let (words, count, duration): (i64, i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(word_count), 0), COUNT(*), COALESCE(SUM(duration_ms), 0)
             FROM history WHERE created_at >= ?1",
            [week_ago],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
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

        // Typical speech is ~150 wpm and comfortable typing ~40 wpm.
        let typing_ms = words.saturating_mul(1_500);
        Ok(DashboardData {
            words_this_week: words,
            dictations_this_week: count,
            time_dictated_ms: duration,
            estimated_saved_ms: typing_ms.saturating_sub(duration),
            history,
        })
    }

    pub fn insert_history(&self, text: &str, raw_text: &str, duration_ms: i64) -> Result<()> {
        let word_count = text.split_whitespace().count() as i64;
        self.conn()?.execute(
            "INSERT INTO history(text, raw_text, word_count, duration_ms, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![text, raw_text, word_count, duration_ms, Self::now()],
        )?;
        Ok(())
    }

    pub fn dictionary(&self) -> Result<Vec<DictionaryEntry>> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT id, value, created_at FROM dictionary ORDER BY value COLLATE NOCASE",
        )?;
        let entries = statement
            .query_map([], |row| {
                Ok(DictionaryEntry {
                    id: row.get(0)?,
                    value: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn add_dictionary(&self, value: &str) -> Result<DictionaryEntry> {
        let value = value.trim();
        if value.is_empty() {
            return Err(FlowError::Message("Enter a word or name.".into()));
        }
        let now = Self::now();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO dictionary(value, created_at) VALUES(?1, ?2)",
            params![value, now],
        )
        .map_err(|error| match error {
            rusqlite::Error::SqliteFailure(ref failure, _) if failure.extended_code == 2067 => {
                FlowError::Message("That dictionary entry already exists.".into())
            }
            other => FlowError::Database(other),
        })?;
        Ok(DictionaryEntry {
            id: conn.last_insert_rowid(),
            value: value.into(),
            created_at: now,
        })
    }

    pub fn update_dictionary(&self, id: i64, value: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE dictionary SET value = ?1 WHERE id = ?2",
            params![value.trim(), id],
        )?;
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
        if trigger.is_empty() || content.trim().is_empty() {
            return Err(FlowError::Message(
                "A snippet needs both a trigger and content.".into(),
            ));
        }
        let now = Self::now();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO snippets(trigger, content, created_at) VALUES(?1, ?2, ?3)",
            params![trigger, content, now],
        )
        .map_err(|error| match error {
            rusqlite::Error::SqliteFailure(ref failure, _) if failure.extended_code == 2067 => {
                FlowError::Message("That snippet trigger already exists.".into())
            }
            other => FlowError::Database(other),
        })?;
        Ok(Snippet {
            id: conn.last_insert_rowid(),
            trigger: trigger.into(),
            content: content.into(),
            created_at: now,
        })
    }

    pub fn update_snippet(&self, id: i64, trigger: &str, content: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE snippets SET trigger = ?1, content = ?2 WHERE id = ?3",
            params![trigger.trim(), content, id],
        )?;
        Ok(())
    }

    pub fn delete_snippet(&self, id: i64) -> Result<()> {
        self.conn()?
            .execute("DELETE FROM snippets WHERE id = ?1", [id])?;
        Ok(())
    }
}
