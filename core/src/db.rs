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

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
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
    // v3 — OCR pipeline bookkeeping
    r#"
    ALTER TABLE screenshots ADD COLUMN ocr_attempts INTEGER NOT NULL DEFAULT 0;
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
    /// OCR queued or not yet attempted.
    pub ocr_pending: i64,
    /// OCR currently being processed by a worker.
    pub ocr_processing: i64,
    pub problem_count: i64,
    /// Oldest capture timestamp (unix seconds), if any.
    pub oldest_ts: Option<i64>,
    /// Newest capture timestamp (unix seconds), if any.
    pub newest_ts: Option<i64>,
}

/// A tag with its usage count, for the sidebar / filter UI.
#[derive(Debug, Clone, Serialize)]
pub struct TagInfo {
    pub name: String,
    pub count: i64,
}

/// A collection with its item count, for the sidebar / manager UI.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionInfo {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub item_count: i64,
    pub created_at: String,
}

/// Map the shared grid-row column list to a `ScreenshotRow`.
/// Column order must match: id, path, filename, created_ts, width, height,
/// format, status, ocr_status, content_hash, phash, starred.
pub(crate) fn map_screenshot_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScreenshotRow> {
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
}

/// Normalize a user-entered tag name: trimmed, non-empty, capped length.
/// Returns None for names that carry no meaning (empty/whitespace).
/// Case is preserved for display; uniqueness is NOCASE at the schema level.
fn normalize_tag_name(name: &str) -> Option<String> {
    const MAX_TAG_LEN: usize = 64;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = trimmed.to_string();
    if out.chars().count() > MAX_TAG_LEN {
        out = out.chars().take(MAX_TAG_LEN).collect();
    }
    Some(out)
}

/// Normalize a user-entered collection name. Same rules as tags but with a
/// longer cap since collection names appear as sidebar headings.
fn normalize_collection_name(name: &str) -> Option<String> {
    const MAX_COLLECTION_LEN: usize = 120;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = trimmed.to_string();
    if out.chars().count() > MAX_COLLECTION_LEN {
        out = out.chars().take(MAX_COLLECTION_LEN).collect();
    }
    Some(out)
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

/// Full record for the viewer: grid fields + user metadata + source context
/// + OCR text + tags. Returned even when the file itself is missing.
#[derive(Debug, Clone, Serialize)]
pub struct ScreenshotDetail {
    pub id: i64,
    pub path: String,
    pub filename: String,
    pub created_ts: Option<i64>,
    pub modified_ts: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub format: Option<String>,
    pub content_hash: Option<String>,
    pub phash: Option<String>,
    pub status: String,
    pub ocr_status: String,
    pub starred: bool,
    pub read_later: bool,
    pub note: String,
    pub app_name: Option<String>,
    pub website_domain: Option<String>,
    pub url: Option<String>,
    pub window_title: Option<String>,
    pub category: Option<String>,
    pub category_confidence: Option<f64>,
    pub ocr_text: Option<String>,
    pub ocr_confidence: Option<f32>,
    pub tags: Vec<String>,
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
                 ocr_status = 'queued',
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

    /// Mark a single record missing by exact path (file watcher).
    pub fn mark_missing_by_path(&self, path: &str) -> CoreResult<usize> {
        Ok(self.conn.execute(
            "UPDATE screenshots SET status = 'missing' WHERE path = ?1",
            params![path],
        )?)
    }

    /// Hash-based identity: if a file with this content hash was previously
    /// marked missing (renamed/moved/restored), re-point that record to its
    /// new location instead of creating a duplicate record. Existing OCR
    /// stays valid because the content is byte-identical.
    pub fn restore_missing_record(
        &self,
        content_hash: &str,
        new_path: &str,
        new_filename: &str,
        size: i64,
        modified_ts: Option<i64>,
    ) -> CoreResult<Option<i64>> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM screenshots
                 WHERE content_hash = ?1 AND status = 'missing'
                 ORDER BY id LIMIT 1",
                params![content_hash],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        self.conn.execute(
            "UPDATE screenshots
             SET path = ?2, filename = ?3, size = ?4, modified_ts = ?5,
                 status = 'available', last_verified_at = datetime('now')
             WHERE id = ?1",
            params![id, new_path, new_filename, size, modified_ts],
        )?;
        Ok(Some(id))
    }

    /// Full record for the viewer: core row + user/source metadata + OCR text
    /// + tags. Missing files still resolve (metadata survives deletion).
    pub fn get_screenshot_detail(&self, id: i64) -> CoreResult<Option<ScreenshotDetail>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, path, filename, created_ts, modified_ts, width, height,
                        format, content_hash, phash, status, ocr_status, starred,
                        read_later, note, app_name, website_domain, url, window_title,
                        category, category_confidence
                 FROM screenshots WHERE id = ?1",
                params![id],
                |r| {
                    Ok(ScreenshotDetail {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        filename: r.get(2)?,
                        created_ts: r.get(3)?,
                        modified_ts: r.get(4)?,
                        width: r.get(5)?,
                        height: r.get(6)?,
                        format: r.get(7)?,
                        content_hash: r.get(8)?,
                        phash: r.get(9)?,
                        status: r.get(10)?,
                        ocr_status: r.get(11)?,
                        starred: r.get::<_, i64>(12)? != 0,
                        read_later: r.get::<_, i64>(13)? != 0,
                        note: r.get(14)?,
                        app_name: r.get(15)?,
                        website_domain: r.get(16)?,
                        url: r.get(17)?,
                        window_title: r.get(18)?,
                        category: r.get(19)?,
                        category_confidence: r.get(20)?,
                        ocr_text: None,
                        ocr_confidence: None,
                        tags: Vec::new(),
                    })
                },
            )
            .optional()?;
        let Some(mut detail) = row else { return Ok(None) };

        detail.ocr_text = self
            .conn
            .query_row(
                "SELECT text, confidence FROM ocr_text WHERE screenshot_id = ?1",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<f32>>(1)?)),
            )
            .optional()?
            .map(|(t, c)| {
                detail.ocr_confidence = c;
                t
            });

        let mut stmt = self.conn.prepare(
            "SELECT t.name FROM tags t
             JOIN screenshot_tags st ON st.tag_id = t.id
             WHERE st.screenshot_id = ?1 ORDER BY t.name",
        )?;
        detail.tags = stmt
            .query_map(params![id], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(detail))
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
        stats.ocr_pending = get(
            "SELECT COUNT(*) FROM screenshots WHERE ocr_status IN ('none', 'queued')",
        )?;
        stats.ocr_processing = get(
            "SELECT COUNT(*) FROM screenshots WHERE ocr_status = 'processing'",
        )?;
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
            .query_map(params![limit, offset], map_screenshot_row)?
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

    /// One on-disk path for a content hash (any available record; duplicates
    /// share bytes so any copy decodes identically). Used to (re)generate
    /// thumbnails on demand for sizes the scanner didn't pre-render.
    pub fn path_by_content_hash(&self, content_hash: &str) -> CoreResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT path FROM screenshots
                 WHERE content_hash = ?1 AND status = 'available'
                 ORDER BY id LIMIT 1",
                params![content_hash],
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

    /// Read-only accessor for higher-level engines (search) built on top of
    /// the same connection.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    // ---- OCR pipeline support -------------------------------------------

    /// Claim the next pending OCR job (atomic even across connections).
    /// `exclude` lists ids already attempted in this pipeline run, so a
    /// job re-queued after a failure is retried on the *next* run instead
    /// of being re-claimed forever within the same one.
    /// Returns (screenshot_id, path) or None when the queue is empty.
    pub fn claim_ocr_job(
        &self,
        max_attempts: i64,
        exclude: &[i64],
    ) -> CoreResult<Option<(i64, String)>> {
        let mut sql = String::from(
            "SELECT id FROM screenshots
             WHERE ocr_status IN ('none', 'queued')
               AND status = 'available'
               AND ocr_attempts < ?1",
        );
        let mut bind: Vec<rusqlite::types::Value> = vec![max_attempts.into()];
        if !exclude.is_empty() {
            let placeholders: Vec<String> = (0..exclude.len())
                .map(|i| format!("?{}", i + 2))
                .collect();
            sql.push_str(&format!(" AND id NOT IN ({})", placeholders.join(", ")));
            bind.extend(exclude.iter().map(|&id| id.into()));
        }
        sql.push_str(" ORDER BY id LIMIT 1");
        let candidate: Option<i64> = self
            .conn
            .prepare(&sql)?
            .query_row(params_from_iter(bind.iter()), |r| r.get(0))
            .optional()?;
        let Some(id) = candidate else {
            return Ok(None);
        };
        // Atomically transition none/queued -> processing so concurrent
        // workers never process the same screenshot twice.
        let claimed = self.conn.execute(
            "UPDATE screenshots SET ocr_status = 'processing'
             WHERE id = ?1 AND ocr_status IN ('none', 'queued')",
            params![id],
        )?;
        if claimed == 0 {
            return Ok(None);
        }
        let path: String = self.conn.query_row(
            "SELECT path FROM screenshots WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        Ok(Some((id, path)))
    }

    /// How many jobs the pipeline could claim right now — the accurate
    /// denominator for OCR progress (excludes missing files and parked
    /// failures, unlike `library_stats().ocr_pending`).
    pub fn ocr_claimable_count(&self, max_attempts: i64) -> CoreResult<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM screenshots
             WHERE ocr_status IN ('none', 'queued')
               AND status = 'available'
               AND ocr_attempts < ?1",
            params![max_attempts],
            |r| r.get(0),
       )?)
    }

    /// Persist successful OCR output and make it searchable immediately.
    pub fn save_ocr_text(
        &self,
        screenshot_id: i64,
        text: &str,
        confidence: Option<f32>,
        engine_version: &str,
    ) -> CoreResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO ocr_text(screenshot_id, text, confidence, engine_version)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(screenshot_id) DO UPDATE SET
                 text = excluded.text,
                 confidence = excluded.confidence,
                 engine_version = excluded.engine_version,
                 extracted_at = datetime('now')",
            params![screenshot_id, text, confidence, engine_version],
        )?;
        tx.execute(
            "UPDATE screenshots SET ocr_status = 'done', last_verified_at = datetime('now')
             WHERE id = ?1",
            params![screenshot_id],
        )?;
        tx.commit()?;
        // FTS update after commit so the text is searchable right away.
        self.fts_set_ocr(screenshot_id, text)?;
        Ok(())
    }

    /// Record an OCR failure (with attempt count) so it can be retried.
    pub fn record_ocr_failure(&self, screenshot_id: i64, max_attempts: i64) -> CoreResult<()> {
        self.conn.execute(
            "UPDATE screenshots
             SET ocr_attempts = ocr_attempts + 1,
                 ocr_status = CASE WHEN ocr_attempts + 1 >= ?2
                                   THEN 'failed' ELSE 'queued' END
             WHERE id = ?1",
            params![screenshot_id, max_attempts],
        )?;
        Ok(())
    }

    /// Re-queue every failed OCR job (user-triggered retry).
    pub fn retry_failed_ocr(&self) -> CoreResult<usize> {
        Ok(self.conn.execute(
            "UPDATE screenshots SET ocr_status = 'queued' WHERE ocr_status = 'failed'",
            [],
        )?)
    }

    /// Enqueue OCR for a single screenshot (used by the file watcher).
    pub fn queue_ocr(&self, screenshot_id: i64) -> CoreResult<()> {
        self.conn.execute(
            "UPDATE screenshots SET ocr_status = 'queued' WHERE id = ?1",
            params![screenshot_id],
        )?;
        Ok(())
    }

    /// True while the record still refers to an existing, available file.
    pub fn ocr_candidate(&self, screenshot_id: i64) -> CoreResult<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM screenshots WHERE id = ?1 AND status = 'available'",
                params![screenshot_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }

    // ---- Organize: tags, flags, notes -------------------------------------

    /// Attach a tag to a screenshot (idempotent). The tag row is created on
    /// demand; the FTS index is re-synced so `tag:name` search works at once.
    /// Returns false when the screenshot does not exist.
    pub fn add_tag(&self, screenshot_id: i64, name: &str) -> CoreResult<bool> {
        let Some(tag) = normalize_tag_name(name) else {
            return Err(crate::error::CoreError::other(
                "tag name must not be empty",
            ));
        };
        if !self.screenshot_exists(screenshot_id)? {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO tags(name) VALUES (?1)
             ON CONFLICT(name) DO NOTHING",
            params![tag],
        )?;
        self.conn.execute(
            "INSERT INTO screenshot_tags(screenshot_id, tag_id, origin)
             SELECT ?1, id, 'manual' FROM tags WHERE name = ?2
             ON CONFLICT(screenshot_id, tag_id) DO NOTHING",
            params![screenshot_id, tag],
        )?;
        self.fts_sync_tags(screenshot_id)?;
        Ok(true)
    }

    /// Detach a tag from a screenshot. Orphan tag rows are pruned so the
    /// sidebar never fills with unused names. Returns false when nothing
    /// was attached.
    pub fn remove_tag(&self, screenshot_id: i64, name: &str) -> CoreResult<bool> {
        let Some(tag) = normalize_tag_name(name) else {
            return Ok(false);
        };
        let removed = self.conn.execute(
            "DELETE FROM screenshot_tags
             WHERE screenshot_id = ?1
               AND tag_id IN (SELECT id FROM tags WHERE name = ?2 COLLATE NOCASE)",
            params![screenshot_id, tag],
        )?;
        // Prune tags no screenshot references anymore.
        self.conn.execute(
            "DELETE FROM tags
             WHERE id NOT IN (SELECT tag_id FROM screenshot_tags)",
            [],
        )?;
        if removed > 0 {
            self.fts_sync_tags(screenshot_id)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Every tag in use with its screenshot count (for the sidebar).
    pub fn list_tags(&self) -> CoreResult<Vec<TagInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name, COUNT(st.screenshot_id) AS n
             FROM tags t
             JOIN screenshot_tags st ON st.tag_id = t.id
             GROUP BY t.id
             ORDER BY n DESC, t.name COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TagInfo {
                    name: r.get(0)?,
                    count: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Recompute the space-joined tag list in the FTS index for one row.
    /// Called after every tag mutation; tag renames flow through search via
    /// the `tag:` filter's direct table lookup, but free-text matches read
    /// this denormalized column.
    pub fn fts_sync_tags(&self, screenshot_id: i64) -> CoreResult<()> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name FROM tags t
             JOIN screenshot_tags st ON st.tag_id = t.id
             WHERE st.screenshot_id = ?1 ORDER BY t.name",
        )?;
        let names = stmt
            .query_map(params![screenshot_id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        self.conn.execute(
            "UPDATE fts_search SET tags = ?2 WHERE screenshot_id = ?1",
            params![screenshot_id, names.join(" ")],
        )?;
        Ok(())
    }

    /// Star or unstar a screenshot. Returns false for unknown ids.
    pub fn set_starred(&self, screenshot_id: i64, starred: bool) -> CoreResult<bool> {
        Ok(self.conn.execute(
            "UPDATE screenshots SET starred = ?2 WHERE id = ?1",
            params![screenshot_id, i64::from(starred)],
        )? > 0)
    }

    /// Toggle the read-later flag. Returns false for unknown ids.
    pub fn set_read_later(&self, screenshot_id: i64, read_later: bool) -> CoreResult<bool> {
        Ok(self.conn.execute(
            "UPDATE screenshots SET read_later = ?2 WHERE id = ?1",
            params![screenshot_id, i64::from(read_later)],
        )? > 0)
    }

    /// Replace the free-text note (synced to FTS by the existing trigger).
    /// Returns false for unknown ids.
    pub fn set_note(&self, screenshot_id: i64, note: &str) -> CoreResult<bool> {
        const MAX_NOTE_LEN: usize = 10_000;
        let mut text = note.trim().to_string();
        if text.chars().count() > MAX_NOTE_LEN {
            text = text.chars().take(MAX_NOTE_LEN).collect();
        }
        Ok(self.conn.execute(
            "UPDATE screenshots SET note = ?2 WHERE id = ?1",
            params![screenshot_id, text],
        )? > 0)
    }

    fn screenshot_exists(&self, screenshot_id: i64) -> CoreResult<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM screenshots WHERE id = ?1",
                params![screenshot_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }

    // ---- Organize: collections --------------------------------------------

    /// Create a manual collection. Names are unique; creating a duplicate
    /// returns the existing row (idempotent, like `add_directory`).
    pub fn create_collection(&self, name: &str) -> CoreResult<CollectionInfo> {
        let Some(clean) = normalize_collection_name(name) else {
            return Err(crate::error::CoreError::other(
                "collection name must not be empty",
            ));
        };
        self.conn.execute(
            "INSERT INTO collections(name, type) VALUES (?1, 'manual')
             ON CONFLICT(name) DO NOTHING",
            params![clean],
        )?;
        self.collection_by_name(&clean)?
            .ok_or_else(|| crate::error::CoreError::other("collection missing"))
    }

    /// Rename a collection. Returns false for unknown ids; errors on a
    /// name clash with another collection.
    pub fn rename_collection(&self, id: i64, name: &str) -> CoreResult<bool> {
        let Some(clean) = normalize_collection_name(name) else {
            return Err(crate::error::CoreError::other(
                "collection name must not be empty",
            ));
        };
        Ok(self.conn.execute(
            "UPDATE collections SET name = ?2 WHERE id = ?1",
            params![id, clean],
        )? > 0)
    }

    /// Delete a collection (items cascade; screenshots are untouched).
    pub fn delete_collection(&self, id: i64) -> CoreResult<bool> {
        Ok(self.conn.execute("DELETE FROM collections WHERE id = ?1", params![id])? > 0)
    }

    /// Every collection with its item count, newest first.
    pub fn list_collections(&self) -> CoreResult<Vec<CollectionInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.name, c.type, COUNT(ci.screenshot_id) AS n, c.created_at
             FROM collections c
             LEFT JOIN collection_items ci ON ci.collection_id = c.id
             GROUP BY c.id
             ORDER BY c.created_at DESC, c.id DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(CollectionInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    item_count: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn collection_by_name(&self, name: &str) -> CoreResult<Option<CollectionInfo>> {
        Ok(self
            .conn
            .query_row(
                "SELECT c.id, c.name, c.type, COUNT(ci.screenshot_id), c.created_at
                 FROM collections c
                 LEFT JOIN collection_items ci ON ci.collection_id = c.id
                 WHERE c.name = ?1
                 GROUP BY c.id",
                params![name],
                |r| {
                    Ok(CollectionInfo {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        kind: r.get(2)?,
                        item_count: r.get(3)?,
                        created_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Add a screenshot to a collection (idempotent). Returns false when
    /// either side does not exist.
    pub fn add_to_collection(&self, collection_id: i64, screenshot_id: i64) -> CoreResult<bool> {
        if !self.screenshot_exists(screenshot_id)? {
            return Ok(false);
        }
        let collection: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM collections WHERE id = ?1",
                params![collection_id],
                |r| r.get(0),
            )
            .optional()?;
        if collection.is_none() {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO collection_items(collection_id, screenshot_id)
             VALUES (?1, ?2)
             ON CONFLICT(collection_id, screenshot_id) DO NOTHING",
            params![collection_id, screenshot_id],
        )?;
        Ok(true)
    }

    /// Remove a screenshot from a collection. Returns false when it was
    /// not a member.
    pub fn remove_from_collection(
        &self,
        collection_id: i64,
        screenshot_id: i64,
    ) -> CoreResult<bool> {
        Ok(self.conn.execute(
            "DELETE FROM collection_items WHERE collection_id = ?1 AND screenshot_id = ?2",
            params![collection_id, screenshot_id],
        )? > 0)
    }

    /// Paged items of a collection, newest first (same row shape as the grid).
    pub fn list_collection_items(
        &self,
        collection_id: i64,
        limit: i64,
        offset: i64,
    ) -> CoreResult<Vec<ScreenshotRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.path, s.filename, s.created_ts, s.width, s.height, s.format,
                    s.status, s.ocr_status, s.content_hash, s.phash, s.starred
             FROM screenshots s
             JOIN collection_items ci ON ci.screenshot_id = s.id
             WHERE ci.collection_id = ?1
             ORDER BY COALESCE(s.created_ts, s.modified_ts) DESC, s.id DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(params![collection_id, limit, offset], map_screenshot_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Collections a screenshot belongs to (for the detail editor).
    pub fn screenshot_collections(&self, screenshot_id: i64) -> CoreResult<Vec<CollectionInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.name, c.type,
                    (SELECT COUNT(*) FROM collection_items WHERE collection_id = c.id),
                    c.created_at
             FROM collections c
             JOIN collection_items ci ON ci.collection_id = c.id
             WHERE ci.screenshot_id = ?1
             ORDER BY c.name COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map(params![screenshot_id], |r| {
                Ok(CollectionInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    item_count: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
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

    #[test]
    fn path_by_content_hash_finds_available_record() {
        let db = Database::open_in_memory().unwrap();
        db.insert_screenshot(&NewScreenshot {
            path: "/tmp/shot.png".into(),
            filename: "shot.png".into(),
            content_hash: Some("abc123".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            db.path_by_content_hash("abc123").unwrap().as_deref(),
            Some("/tmp/shot.png")
        );
        assert_eq!(db.path_by_content_hash("nope").unwrap(), None);
    }

    fn organize_fixture() -> (Database, i64) {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .insert_screenshot(&NewScreenshot {
                path: "/tmp/vacation.png".into(),
                filename: "vacation.png".into(),
                ..Default::default()
            })
            .unwrap();
        (db, id)
    }

    #[test]
    fn tags_add_remove_list_and_fts() {
        let (db, id) = organize_fixture();
        assert!(db.add_tag(id, "  Travel ").unwrap());
        assert!(db.add_tag(id, "travel").unwrap(), "idempotent");
        assert!(db.add_tag(id, "beach").unwrap());
        assert!(db.add_tag(id, "   ").is_err(), "blank tag rejected");
        assert!(!db.add_tag(9999, "ghost").unwrap(), "unknown id");

        let tags = db.list_tags().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].count, 1);

        let detail = db.get_screenshot_detail(id).unwrap().unwrap();
        // Display case of first insert is preserved (uniqueness is NOCASE).
        assert_eq!(detail.tags, vec!["beach".to_string(), "Travel".to_string()]);

        // FTS denormalized column follows tag mutations.
        let fts_tags: String = db
            .conn
            .query_row(
                "SELECT tags FROM fts_search WHERE screenshot_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        let fts_lower = fts_tags.to_ascii_lowercase();
        assert!(fts_lower.contains("beach") && fts_lower.contains("travel"));

        assert!(db.remove_tag(id, "BEACH").unwrap(), "NOCASE removal");
        assert!(!db.remove_tag(id, "beach").unwrap(), "already gone");
        assert_eq!(db.list_tags().unwrap().len(), 1, "orphan pruned");
        let detail = db.get_screenshot_detail(id).unwrap().unwrap();
        assert_eq!(detail.tags, vec!["Travel".to_string()]);
    }

    #[test]
    fn flags_and_note_roundtrip() {
        let (db, id) = organize_fixture();
        assert!(db.set_starred(id, true).unwrap());
        assert!(db.set_read_later(id, true).unwrap());
        assert!(db.set_note(id, "  trip ideas  ").unwrap());
        assert!(!db.set_starred(9999, true).unwrap());

        let detail = db.get_screenshot_detail(id).unwrap().unwrap();
        assert!(detail.starred);
        assert!(detail.read_later);
        assert_eq!(detail.note, "trip ideas");

        // Note text lands in FTS via the update trigger.
        let hits: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM fts_search WHERE fts_search MATCH 'note:trip'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);

        assert!(db.set_starred(id, false).unwrap());
        assert!(!db.get_screenshot_detail(id).unwrap().unwrap().starred);
    }

    #[test]
    fn collections_crud_and_items() {
        let (db, id) = organize_fixture();
        let other = db
            .insert_screenshot(&NewScreenshot {
                path: "/tmp/work.png".into(),
                filename: "work.png".into(),
                ..Default::default()
            })
            .unwrap();

        let c = db.create_collection("  Trips ").unwrap();
        assert_eq!(c.name, "Trips");
        let again = db.create_collection("Trips").unwrap();
        assert_eq!(c.id, again.id, "idempotent");
        assert!(db.create_collection("  ").is_err());

        assert!(db.add_to_collection(c.id, id).unwrap());
        assert!(db.add_to_collection(c.id, id).unwrap(), "idempotent");
        assert!(!db.add_to_collection(c.id, 9999).unwrap());
        assert!(!db.add_to_collection(9999, id).unwrap());

        let cols = db.list_collections().unwrap();
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].item_count, 1);

        let items = db.list_collection_items(c.id, 10, 0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, id);

        assert!(db.remove_from_collection(c.id, other).unwrap() == false);
        assert!(db.remove_from_collection(c.id, id).unwrap());
        assert!(db.list_collection_items(c.id, 10, 0).unwrap().is_empty());

        assert!(db.rename_collection(c.id, "Holidays").unwrap());
        assert_eq!(db.list_collections().unwrap()[0].name, "Holidays");
        assert!(db.delete_collection(c.id).unwrap());
        assert!(db.list_collections().unwrap().is_empty());
        // Screenshots survive collection deletion.
        assert!(db.get_screenshot_detail(id).unwrap().is_some());
    }
}







