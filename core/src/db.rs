//! SQLite persistence layer.
//!
//! One embedded database holds everything: screenshot records, directory
//! configuration, OCR text, tags, collections, saved searches, the FTS5
//! search index, the job queue, and per-file problems. The app is fully
//! functional offline; there is no server anywhere.
//!
//! Write safety: WAL journal mode + transactions around batch writes, so a
//! crash mid-scan or mid-OCR never corrupts the index. Schema changes go
//! through versioned migrations recorded in `schema_migrations`.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::CoreResult;

/// Status values for a screenshot record relative to the filesystem.
pub const STATUS_AVAILABLE: &str = "available";
pub const STATUS_MISSING: &str = "missing";
pub const STATUS_CHANGED: &str = "changed";
pub const STATUS_PENDING: &str = "pending";

/// Versioned schema migrations. Each entry runs once, inside a transaction.
const MIGRATIONS: &[&str] = &[
    // v1 — initial schema
    r#"
    CREATE TABLE directories (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        enabled INTEGER NOT NULL DEFAULT 1,
        added_at TEXT NOT NULL DEFAULT (datetime('now')),
        last_scan_cursor TEXT
    );

    CREATE TABLE screenshots (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        filename TEXT NOT NULL,
        size INTEGER NOT NULL DEFAULT 0,
        created_ts INTEGER,
        modified_ts INTEGER,
        width INTEGER,
        height INTEGER,
        format TEXT,
        content_hash TEXT,
        phash TEXT,
        status TEXT NOT NULL DEFAULT 'available'
            CHECK (status IN ('available', 'missing', 'changed', 'pending')),
        ocr_status TEXT NOT NULL DEFAULT 'none'
            CHECK (ocr_status IN ('none', 'queued', 'processing', 'done', 'failed', 'skipped')),
        source_dir_id INTEGER REFERENCES directories(id) ON DELETE SET NULL,
        app_name TEXT,
        website_domain TEXT,
        url TEXT,
        window_title TEXT,
        category TEXT,
        category_confidence REAL,
        starred INTEGER NOT NULL DEFAULT 0,
        read_later INTEGER NOT NULL DEFAULT 0,
        note TEXT NOT NULL DEFAULT '',
        indexed_at TEXT NOT NULL DEFAULT (datetime('now')),
        last_verified_at TEXT
    );
    CREATE INDEX idx_screenshots_content_hash ON screenshots(content_hash);
    CREATE INDEX idx_screenshots_phash ON screenshots(phash);
    CREATE INDEX idx_screenshots_created_ts ON screenshots(created_ts);
    CREATE INDEX idx_screenshots_status ON screenshots(status);
    CREATE INDEX idx_screenshots_source_dir ON screenshots(source_dir_id);

    CREATE TABLE ocr_text (
        screenshot_id INTEGER PRIMARY KEY REFERENCES screenshots(id) ON DELETE CASCADE,
        text TEXT NOT NULL DEFAULT '',
        language TEXT,
        confidence REAL,
        engine_version TEXT,
        extracted_at TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE TABLE tags (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE COLLATE NOCASE
    );
    CREATE TABLE screenshot_tags (
        screenshot_id INTEGER NOT NULL REFERENCES screenshots(id) ON DELETE CASCADE,
        tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
        origin TEXT NOT NULL DEFAULT 'manual' CHECK (origin IN ('manual', 'suggested', 'auto')),
        PRIMARY KEY (screenshot_id, tag_id)
    );

    CREATE TABLE collections (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        type TEXT NOT NULL DEFAULT 'manual' CHECK (type IN ('manual', 'smart', 'auto')),
        rule_json TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE TABLE collection_items (
        collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
        screenshot_id INTEGER NOT NULL REFERENCES screenshots(id) ON DELETE CASCADE,
        added_at TEXT NOT NULL DEFAULT (datetime('now')),
        PRIMARY KEY (collection_id, screenshot_id)
    );
    "#,
    // v2 — saved searches, jobs, problems, settings, FTS5 index + triggers
    r#"
    CREATE TABLE saved_searches (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        query TEXT NOT NULL,
        filters_json TEXT,
        created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE TABLE jobs (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        payload_json TEXT,
        status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'done', 'failed')),
        attempts INTEGER NOT NULL DEFAULT 0,
        scheduled_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT
    );

    CREATE TABLE problems (
        id INTEGER PRIMARY KEY,
        path TEXT,
        kind TEXT NOT NULL,
        message TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);

    -- Standalone FTS5 index. The scanner immediately makes records findable by
    -- filename; OCR/tags/notes columns are filled in as indexing progresses.
    CREATE VIRTUAL TABLE fts_search USING fts5(
        screenshot_id UNINDEXED,
        filename,
        ocr_text,
        tags,
        note,
        app,
        site
    );

    CREATE TRIGGER screenshots_fts_ai AFTER INSERT ON screenshots BEGIN
        INSERT INTO fts_search(screenshot_id, filename, ocr_text, tags, note, app, site)
        VALUES (new.id, new.filename, '', '', new.note,
                ifnull(new.app_name, ''), ifnull(new.website_domain, ''));
    END;

    CREATE TRIGGER screenshots_fts_ad AFTER DELETE ON screenshots BEGIN
        DELETE FROM fts_search WHERE screenshot_id = old.id;
    END;

    -- Only touch the FTS row when user-visible text columns change; status /
    -- verification updates must not clobber OCR text already in the index.
    -- In-place UPDATE preserves the ocr_text/tags columns.
    CREATE TRIGGER screenshots_fts_au AFTER UPDATE OF filename, note, app_name, website_domain ON screenshots BEGIN
        UPDATE fts_search SET
            filename = new.filename,
            note = new.note,
            app = ifnull(new.app_name, ''),
            site = ifnull(new.website_domain, '')
        WHERE screenshot_id = new.id;
    END;
    "#,
];

/// A configured source directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Directory {
    pub id: i64,
    pub path: String,
    pub enabled: bool,
}

/// The cheap fingerprint used to skip unchanged files on re-scan.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub id: i64,
    pub size: i64,
    pub modified_ts: Option<i64>,
    pub status: String,
}

/// A new screenshot record to insert.
#[derive(Debug, Clone, Default)]
pub struct NewScreenshot {
    pub path: String,
    pub filename: String,
    pub size: i64,
    pub created_ts: Option<i64>,
    pub modified_ts: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub format: Option<String>,
    pub content_hash: Option<String>,
    pub phash: Option<String>,
    pub source_dir_id: Option<i64>,
}

/// Aggregate library statistics for the status bar / index health screen.
#[derive(Debug, Clone, Serialize, Default)]
pub struct LibraryStats {
    pub total: i64,
    pub available: i64,
    pub missing: i64,
    pub changed: i64,
    pub pending: i64,
    pub with_ocr: i64,
    pub ocr_failed: i64,
    pub problem_count: i64,
    /// Oldest capture timestamp (unix seconds), if any.
    pub oldest_ts: Option<i64>,
    /// Newest capture timestamp (unix seconds), if any.
    pub newest_ts: Option<i64>,
}

/// A recorded per-file problem (corrupted image, unreadable file, ...).
#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub id: i64,
    pub path: Option<String>,
    pub kind: String,
    pub message: String,
    pub created_at: String,
}

/// A screenshot record as returned to the UI grid.
#[derive(Debug, Clone, Serialize)]
pub struct ScreenshotRow {
    pub id: i64,
    pub path: String,
    pub filename: String,
    pub created_ts: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub format: Option<String>,
    pub status: String,
    pub ocr_status: String,
    pub content_hash: Option<String>,
    pub phash: Option<String>,
    pub starred: bool,
}

/// Embedded SQLite database handle.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (and migrate) the database file. WAL mode keeps readers and the
    /// background scanner from blocking each other.
    pub fn open(path: &Path) -> CoreResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// In-memory database (tests).
    pub fn open_in_memory() -> CoreResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> CoreResult<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> CoreResult<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )?;
        let current: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let version = (i + 1) as i64;
            if version <= current {
                continue;
            }
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(version) VALUES (?1)",
                params![version],
            )?;
            tx.commit()?;
            log::info!("applied schema migration v{version}");
        }
        Ok(())
    }

    /// Begin a batch write transaction. The scanner commits periodically so a
    /// crash never loses more than one batch, and partial progress is kept
    /// (fingerprinting makes the remainder cheap to redo).
    pub fn begin_batch(&self) -> CoreResult<rusqlite::Transaction<'_>> {
        Ok(self.conn.unchecked_transaction()?)
    }

    /// Register a source directory. Idempotent on path.
    pub fn add_directory(&self, path: &Path) -> CoreResult<Directory> {
        let canonical = normalize_path(path);
        self.conn.execute(
            "INSERT INTO directories(path) VALUES (?1) ON CONFLICT(path) DO NOTHING",
            params![canonical],
        )?;
        let row = self.conn.query_row(
            "SELECT id, path, enabled FROM directories WHERE path = ?1",
            params![canonical],
            |r| {
                Ok(Directory {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    enabled: r.get::<_, i64>(2)? != 0,
                })
            },
        )?;
        Ok(row)
    }

    pub fn list_directories(&self) -> CoreResult<Vec<Directory>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, enabled FROM directories ORDER BY path")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Directory {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    enabled: r.get::<_, i64>(2)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn remove_directory(&self, id: i64) -> CoreResult<()> {
        self.conn
            .execute("DELETE FROM directories WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Look up a record by exact normalized path.
    pub fn fingerprint_by_path(&self, path: &str) -> CoreResult<Option<Fingerprint>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, size, modified_ts, status FROM screenshots WHERE path = ?1",
                params![path],
                |r| {
                    Ok(Fingerprint {
                        id: r.get(0)?,
                        size: r.get(1)?,
                        modified_ts: r.get(2)?,
                        status: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Whether the stored fingerprint (size + modified time) matches disk.
    /// Unchanged files are skipped on re-scan — this is what makes scans
    /// resumable and incremental rather than wasteful.
    pub fn fingerprint_matches(fp: &Fingerprint, size: i64, modified_ts: Option<i64>) -> bool {
        fp.status == STATUS_AVAILABLE && fp.size == size && fp.modified_ts == modified_ts
    }

    /// Insert a screenshot record, or update it in place if a record already
    /// exists at this path (a *changed* file). Stale OCR text is invalidated
    /// because it no longer describes the current content.
    pub fn insert_screenshot(&self, rec: &NewScreenshot) -> CoreResult<i64> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM screenshots WHERE path = ?1",
                params![rec.path],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            self.conn.execute(
                "DELETE FROM ocr_text WHERE screenshot_id = ?1",
                params![id],
            )?;
            self.conn.execute(
                "UPDATE fts_search SET ocr_text = '' WHERE screenshot_id = ?1",
                params![id],
            )?;
        }
        self.conn.execute(
            "INSERT INTO screenshots (
                 path, filename, size, created_ts, modified_ts,
                 width, height, format, content_hash, phash, source_dir_id, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'available')
             ON CONFLICT(path) DO UPDATE SET
                 filename = excluded.filename,
                 size = excluded.size,
                 created_ts = excluded.created_ts,
                 modified_ts = excluded.modified_ts,
                 width = excluded.width,
                 height = excluded.height,
                 format = excluded.format,
                 content_hash = excluded.content_hash,
                 phash = excluded.phash,
                 source_dir_id = excluded.source_dir_id,
                 status = 'available',
                 ocr_status = 'none',
                 indexed_at = datetime('now')",
            params![
                rec.path,
                rec.filename,
                rec.size,
                rec.created_ts,
                rec.modified_ts,
                rec.width,
                rec.height,
                rec.format,
                rec.content_hash,
                rec.phash,
                rec.source_dir_id,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update last_verified_at for a file that was confirmed unchanged.
    pub fn touch_verified(&self, id: i64) -> CoreResult<()> {
        self.conn.execute(
            "UPDATE screenshots SET last_verified_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Mark records as missing (e.g. disconnected drive). Metadata is kept —
    /// the user may reconnect the drive; we never destroy records silently.
    pub fn mark_missing(&self, ids: &[i64]) -> CoreResult<usize> {
        let mut n = 0;
        for id in ids {
            n += self.conn.execute(
                "UPDATE screenshots SET status = 'missing' WHERE id = ?1",
                params![id],
            )?;
        }
        Ok(n)
    }

    /// Put extracted OCR text into the search index for a screenshot
    /// (used by the OCR pipeline once text is available).
    pub fn fts_set_ocr(&self, screenshot_id: i64, text: &str) -> CoreResult<()> {
        self.conn.execute(
            "UPDATE fts_search SET ocr_text = ?2 WHERE screenshot_id = ?1",
            params![screenshot_id, text],
        )?;
        Ok(())
    }

    /// Record a per-file problem. One bad file never halts a scan.
    pub fn record_problem(&self, path: Option<&str>, kind: &str, message: &str) -> CoreResult<()> {
        self.conn.execute(
            "INSERT INTO problems(path, kind, message) VALUES (?1, ?2, ?3)",
            params![path, kind, message],
        )?;
        log::warn!("problem recorded [{kind}]: {message}");
        Ok(())
    }

    pub fn list_problems(&self, limit: i64) -> CoreResult<Vec<Problem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, kind, message, created_at
             FROM problems ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(Problem {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    kind: r.get(2)?,
                    message: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Remove problem entries (e.g. after a successful re-scan).
    pub fn clear_problems(&self) -> CoreResult<()> {
        self.conn.execute("DELETE FROM problems", [])?;
        Ok(())
    }

    /// Aggregate library statistics for the status bar / index health screen.
    pub fn library_stats(&self) -> CoreResult<LibraryStats> {
        let mut stats = LibraryStats::default();
        let get = |sql: &str| -> CoreResult<i64> {
            Ok(self.conn.query_row(sql, [], |r| r.get(0))?)
        };
        stats.total = get("SELECT COUNT(*) FROM screenshots")?;
        stats.available = get("SELECT COUNT(*) FROM screenshots WHERE status = 'available'")?;
        stats.missing = get("SELECT COUNT(*) FROM screenshots WHERE status = 'missing'")?;
        stats.changed = get("SELECT COUNT(*) FROM screenshots WHERE status = 'changed'")?;
        stats.pending = get("SELECT COUNT(*) FROM screenshots WHERE status = 'pending'")?;
        stats.with_ocr = get("SELECT COUNT(*) FROM screenshots WHERE ocr_status = 'done'")?;
        stats.ocr_failed = get("SELECT COUNT(*) FROM screenshots WHERE ocr_status = 'failed'")?;
        stats.problem_count = get("SELECT COUNT(*) FROM problems")?;
        stats.oldest_ts = self
            .conn
            .query_row(
                "SELECT MIN(created_ts) FROM screenshots WHERE created_ts IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        stats.newest_ts = self
            .conn
            .query_row(
                "SELECT MAX(created_ts) FROM screenshots WHERE created_ts IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(stats)
    }

    /// Paged screenshot listing for the library grid (newest first).
    pub fn list_screenshots(&self, limit: i64, offset: i64) -> CoreResult<Vec<ScreenshotRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, filename, created_ts, width, height, format,
                    status, ocr_status, content_hash, phash, starred
             FROM screenshots
             ORDER BY COALESCE(created_ts, modified_ts) DESC, id DESC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt
            .query_map(params![limit, offset], |r| {
                Ok(ScreenshotRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    filename: r.get(2)?,
                    created_ts: r.get(3)?,
                    width: r.get(4)?,
                    height: r.get(5)?,
                    format: r.get(6)?,
                    status: r.get(7)?,
                    ocr_status: r.get(8)?,
                    content_hash: r.get(9)?,
                    phash: r.get(10)?,
                    starred: r.get::<_, i64>(11)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_setting(&self, key: &str) -> CoreResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> CoreResult<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

/// Normalize a path for storage: absolute and lexically cleaned. Does not
/// resolve symlinks or require the target to exist (non-destructive read-only
/// handling; the file may be on a disconnected drive).
pub fn normalize_path(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut clean = PathBuf::new();
    for comp in abs.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                clean.pop();
            }
            c => clean.push(c.as_os_str()),
        }
    }
    clean.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_once() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap(); // idempotent on the same DB
        let v: i64 = db
            .conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
    }

    #[test]
    fn open_on_disk_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("sub dir/screenshots.db");
        drop(Database::open(&db_path).unwrap());
        drop(Database::open(&db_path).unwrap()); // migrations idempotent across runs
    }

    #[test]
    fn directory_crud() {
        let db = Database::open_in_memory().unwrap();
        let d1 = db.add_directory(Path::new("/tmp/Screenshots")).unwrap();
        let d2 = db
            .add_directory(Path::new("/tmp/Pictures with spaces"))
            .unwrap();
        assert_eq!(d1.path, "/tmp/Screenshots");
        let d1b = db.add_directory(Path::new("/tmp/Screenshots")).unwrap();
        assert_eq!(d1.id, d1b.id, "add_directory is idempotent");
        assert_eq!(db.list_directories().unwrap().len(), 2);
        db.remove_directory(d2.id).unwrap();
        assert_eq!(db.list_directories().unwrap().len(), 1);
    }

    #[test]
    fn screenshot_insert_and_fts_filename_search() {
        let db = Database::open_in_memory().unwrap();
        let rec = NewScreenshot {
            path: "/tmp/Screenshots/Screenshot_2026-09-02.png".into(),
            filename: "Screenshot_2026-09-02.png".into(),
            size: 1234,
            modified_ts: Some(1_785_000_000),
            width: Some(1920),
            height: Some(1080),
            format: Some("png".into()),
            content_hash: Some("abc".into()),
            phash: Some("0123456789abcdef".into()),
            ..Default::default()
        };
        let id = db.insert_screenshot(&rec).unwrap();
        assert!(id > 0);

        let fp = db.fingerprint_by_path(&rec.path).unwrap().unwrap();
        assert!(Database::fingerprint_matches(&fp, 1234, Some(1_785_000_000)));
        assert!(!Database::fingerprint_matches(&fp, 999, Some(1_785_000_000)));
        assert!(!Database::fingerprint_matches(&fp, 1234, None));

        // FTS finds by filename immediately after insert
        let hits: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fts_search WHERE fts_search MATCH 'filename:2026'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);
    }

    #[test]
    fn fts_ocr_update_and_rename_preserves_ocr() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .insert_screenshot(&NewScreenshot {
                path: "/tmp/a.png".into(),
                filename: "a.png".into(),
                ..Default::default()
            })
            .unwrap();
        db.fts_set_ocr(id, "docker error traceback").unwrap();

        // Metadata-only UPDATE (a rename) must preserve OCR text in FTS
        db.conn
            .execute(
                "UPDATE screenshots SET filename = 'renamed.png' WHERE id = ?1",
                params![id],
            )
            .unwrap();

        let ocr_hits: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fts_search WHERE fts_search MATCH 'docker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ocr_hits, 1, "OCR text must survive a rename");

        let name_hits: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fts_search WHERE fts_search MATCH 'filename:renamed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name_hits, 1);
    }

    #[test]
    fn stats_problems_and_listing() {
        let db = Database::open_in_memory().unwrap();
        for name in ["a.png", "b.png", "c.png"] {
            db.insert_screenshot(&NewScreenshot {
                path: format!("/tmp/{name}"),
                filename: name.into(),
                ..Default::default()
            })
            .unwrap();
        }
        db.record_problem(Some("/tmp/bad.png"), "unreadable", "corrupted image")
            .unwrap();
        let stats = db.library_stats().unwrap();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.available, 3);
        assert_eq!(stats.problem_count, 1);

        let rows = db.list_screenshots(10, 0).unwrap();
        assert_eq!(rows.len(), 3);
        let paged = db.list_screenshots(2, 0).unwrap();
        assert_eq!(paged.len(), 2);
        assert_eq!(db.list_screenshots(2, 2).unwrap().len(), 1);

        let problems = db.list_problems(10).unwrap();
        assert_eq!(problems[0].kind, "unreadable");
        db.clear_problems().unwrap();
        assert_eq!(db.library_stats().unwrap().problem_count, 0);
    }

    #[test]
    fn settings_roundtrip_and_mark_missing() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_setting("theme").unwrap().is_none());
        db.set_setting("theme", "dark").unwrap();
        db.set_setting("theme", "system").unwrap(); // upsert
        assert_eq!(db.get_setting("theme").unwrap().as_deref(), Some("system"));

        let id = db
            .insert_screenshot(&NewScreenshot {
                path: "/tmp/gone.png".into(),
                filename: "gone.png".into(),
                ..Default::default()
            })
            .unwrap();
        db.mark_missing(&[id]).unwrap();
        let fp = db
            .fingerprint_by_path("/tmp/gone.png")
            .unwrap()
            .unwrap();
        assert_eq!(fp.status, STATUS_MISSING);
    }

    #[test]
    fn normalize_paths() {
        assert_eq!(
            normalize_path(Path::new("/tmp/a/../b/./c.png")),
            "/tmp/b/c.png"
        );
        assert!(normalize_path(Path::new("relative.png")).starts_with('/'));
    }
}







