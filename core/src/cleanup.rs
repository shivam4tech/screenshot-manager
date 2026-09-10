//! Safe deletion: move screenshot files to the OS trash, never unlink.
//!
//! Records are kept (marked `missing`) so metadata, tags, OCR text, and
//! collection memberships survive — the library stays searchable and the
//! trash can be restored from outside the app. Deletion is always an
//! explicit user action surfaced through a confirmation step in the UI.

use serde::Serialize;
use rusqlite::OptionalExtension;

use crate::db::Database;
use crate::error::CoreResult;

/// One file that could not be trashed.
#[derive(Debug, Clone, Serialize)]
pub struct DeleteFailure {
    pub id: i64,
    pub path: Option<String>,
    pub message: String,
}

/// Outcome of a bulk delete request.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeleteSummary {
    /// Files moved to the OS trash (records marked missing).
    pub trashed: usize,
    /// Records whose files were already gone (marked missing, nothing moved).
    pub already_missing: usize,
    /// Ids that could not be processed, with reasons.
    pub failed: Vec<DeleteFailure>,
}
/// Move the given screenshots to the OS trash and mark their records
/// missing. Unknown ids and trash errors are reported, never fatal to the
/// rest of the batch.
pub fn delete_screenshots(db: &Database, ids: &[i64]) -> CoreResult<DeleteSummary> {
    let mut summary = DeleteSummary::default();
    for id in ids {
        let row: Option<(String, String)> = db
            .conn()
            .query_row(
                "SELECT path, status FROM screenshots WHERE id = ?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((path, _status)) = row else {
            summary.failed.push(DeleteFailure {
                id: *id,
                path: None,
                message: "screenshot not found".into(),
            });
            continue;
        };
        if !std::path::Path::new(&path).exists() {
            db.mark_missing(&[*id])?;
            summary.already_missing += 1;
            continue;
        }
        match trash::delete(&path) {
            Ok(()) => {
                db.mark_missing(&[*id])?;
                summary.trashed += 1;
            }
            Err(e) => summary.failed.push(DeleteFailure {
                id: *id,
                path: Some(path),
                message: format!("could not move to trash: {e}"),
            }),
        }
    }
    Ok(summary)
}

/// Outcome of a reviewed cleanup-trash operation.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CleanupTrashSummary {
    /// Files moved to the OS trash (records marked missing).
    pub trashed: usize,
    /// Records whose files were already gone.
    pub already_missing: usize,
    /// Ids that could not be processed, with reasons.
    pub failed: Vec<DeleteFailure>,
    /// Searchable metadata records retained (0 when the setting is off).
    pub memories_retained: usize,
}

/// Reviewed bulk trash for the Cleanup flow: move files to the OS trash via
/// [`delete_screenshots`], then optionally snapshot searchable metadata
/// records. Memories are recorded only for files this call actually moved
/// (pre-existing files absent from `failed`); files already gone beforehand
/// get no record. Memory recording is best-effort — a record failure is
/// logged, never fatal to the trash operation.
pub fn cleanup_trash(
    db: &Database,
    ids: &[i64],
    keep_memory: bool,
    keep_thumbnail: bool,
) -> CoreResult<CleanupTrashSummary> {
    use std::collections::HashSet;
    let mut snapshots = Vec::new();
    if keep_memory {
        for id in ids {
            if let Some(detail) = db.get_screenshot_detail(*id)? {
                let size: i64 = db
                    .conn()
                    .query_row(
                        "SELECT size FROM screenshots WHERE id = ?1",
                        rusqlite::params![id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                let existed = std::path::Path::new(&detail.path).exists();
                snapshots.push((*id, detail, size, existed));
            }
        }
    }
    let summary = delete_screenshots(db, ids)?;
    let failed: HashSet<i64> = summary.failed.iter().map(|f| f.id).collect();
    let mut memories_retained = 0usize;
    if keep_memory {
        for (id, detail, size, existed) in &snapshots {
            if !existed || failed.contains(id) {
                continue;
            }
            match db.record_deleted_memory(detail, *size, keep_thumbnail) {
                Ok(_) => memories_retained += 1,
                Err(e) => log::warn!("could not record deleted memory for {id}: {e}"),
            }
        }
    }
    Ok(CleanupTrashSummary {
        trashed: summary.trashed,
        already_missing: summary.already_missing,
        failed: summary.failed,
        memories_retained,
    })
}

/// Total stored bytes for the given screenshot ids (selection estimates).
pub fn selection_bytes(db: &Database, ids: &[i64]) -> CoreResult<i64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT COALESCE(SUM(size), 0) FROM screenshots WHERE id IN ({})",
        placeholders.join(",")
    );
    let mut stmt = db.conn().prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let total: i64 = stmt.query_row(params.as_slice(), |r| r.get(0))?;
    Ok(total)
}

/// One record that could not be restored.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreFailure {
    pub id: i64,
    pub path: Option<String>,
    pub message: String,
}

/// Outcome of a restore request (cull undo).
#[derive(Debug, Clone, Default, Serialize)]
pub struct RestoreSummary {
    /// Files recovered from the OS trash (records available again).
    pub restored: usize,
    /// Records whose files were already back in place.
    pub already_there: usize,
    /// Ids that could not be recovered, with reasons.
    pub failed: Vec<RestoreFailure>,
}

/// Recover screenshots from the OS trash back to their recorded paths and
/// mark their records available. Used by cull undo. If the trash no longer
/// holds a file (emptied externally), it is reported, not resurrected.
pub fn restore_screenshots(db: &Database, ids: &[i64]) -> CoreResult<RestoreSummary> {
    let mut summary = RestoreSummary::default();
    for id in ids {
        let row: Option<String> = db
            .conn()
            .query_row(
                "SELECT path FROM screenshots WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(path) = row else {
            summary.failed.push(RestoreFailure {
                id: *id,
                path: None,
                message: "screenshot not found".into(),
            });
            continue;
        };
        if std::path::Path::new(&path).exists() {
            db.mark_available(&[*id])?;
            summary.already_there += 1;
            continue;
        }
        let trash_items = match trash::os_limited::list() {
            Ok(items) => items,
            Err(e) => {
                summary.failed.push(RestoreFailure {
                    id: *id,
                    path: Some(path),
                    message: format!("could not read trash: {e}"),
                });
                continue;
            }
        };
        // Newest deletion of this exact path wins (avoids twin restores).
        let mut matches: Vec<_> = trash_items
            .into_iter()
            .filter(|t| t.original_path() == std::path::Path::new(&path))
            .collect();
        matches.sort_by_key(|t| std::cmp::Reverse(t.time_deleted));
        let Some(item) = matches.into_iter().next() else {
            summary.failed.push(RestoreFailure {
                id: *id,
                path: Some(path),
                message: "no longer in trash (emptied?)".into(),
            });
            continue;
        };
        match trash::os_limited::restore_all(vec![item]) {
            Ok(()) if std::path::Path::new(&path).exists() => {
                db.mark_available(&[*id])?;
                summary.restored += 1;
            }
            Ok(()) => summary.failed.push(RestoreFailure {
                id: *id,
                path: Some(path),
                message: "restore reported success but file is missing".into(),
            }),
            Err(e) => summary.failed.push(RestoreFailure {
                id: *id,
                path: Some(path),
                message: format!("restore failed: {e}"),
            }),
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScreenshot;

    fn paint_png(path: &std::path::Path) {
        let img = image::RgbImage::new(60, 40);
        image::DynamicImage::ImageRgb8(img).save(path).unwrap();
    }

    #[test]
    fn delete_trashes_files_and_marks_missing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("shots");
        std::fs::create_dir_all(&src).unwrap();
        paint_png(&src.join("a.png"));
        paint_png(&src.join("b.png"));

        let db = Database::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for name in ["a.png", "b.png"] {
            ids.push(
                db.insert_screenshot(&NewScreenshot {
                    path: src.join(name).to_string_lossy().into_owned(),
                    filename: name.into(),
                    ..Default::default()
                })
                .unwrap(),
            );
        }
        // One tag + collection membership to prove records survive deletion.
        db.add_tag(ids[0], "keep").unwrap();
        let col = db.create_collection("C").unwrap();
        db.add_to_collection(col.id, ids[0]).unwrap();

        let s = delete_screenshots(&db, &[ids[0], ids[1], 9999]).unwrap();
        assert_eq!(s.trashed, 2);
        assert_eq!(s.failed.len(), 1);
        assert!(!src.join("a.png").exists());
        assert!(!src.join("b.png").exists());

        let d = db.get_screenshot_detail(ids[0]).unwrap().unwrap();
        assert_eq!(d.status, crate::db::STATUS_MISSING);
        assert_eq!(d.tags, vec!["keep".to_string()]);
        assert_eq!(db.screenshot_collections(ids[0]).unwrap().len(), 1);
    }

    #[test]
    fn delete_missing_file_marks_record() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .insert_screenshot(&NewScreenshot {
                path: "/tmp/shotmemory-gone/file.png".into(),
                filename: "file.png".into(),
                ..Default::default()
            })
            .unwrap();
        let s = delete_screenshots(&db, &[id]).unwrap();
        assert_eq!(s.already_missing, 1);
        assert_eq!(s.trashed, 0);
    }

    #[test]
    fn delete_then_restore_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("shots");
        std::fs::create_dir_all(&src).unwrap();
        paint_png(&src.join("r.png"));

        let db = Database::open_in_memory().unwrap();
        let id = db
            .insert_screenshot(&NewScreenshot {
                path: src.join("r.png").to_string_lossy().into_owned(),
                filename: "r.png".into(),
                ..Default::default()
            })
            .unwrap();

        let del = delete_screenshots(&db, &[id]).unwrap();
        assert_eq!(del.trashed, 1);
        assert!(!src.join("r.png").exists());

        let res = restore_screenshots(&db, &[id, 4242]).unwrap();
        assert_eq!(res.restored, 1);
        assert_eq!(res.failed.len(), 1, "unknown id reported");
        assert!(src.join("r.png").exists(), "file back in place");
        let d = db.get_screenshot_detail(id).unwrap().unwrap();
        assert_eq!(d.status, crate::db::STATUS_AVAILABLE);

        // Restoring an already-present file is a no-op success.
        let res = restore_screenshots(&db, &[id]).unwrap();
        assert_eq!(res.already_there, 1);
        assert_eq!(res.restored, 0);
    }

    fn memory_fixture() -> (tempfile::TempDir, Database, Vec<i64>) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("shots");
        std::fs::create_dir_all(&src).unwrap();
        paint_png(&src.join("m1.png"));
        paint_png(&src.join("m2.png"));
        let db = Database::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for name in ["m1.png", "m2.png"] {
            ids.push(
                db.insert_screenshot(&NewScreenshot {
                    path: src.join(name).to_string_lossy().into_owned(),
                    filename: name.into(),
                    size: 4800,
                    ..Default::default()
                })
                .unwrap(),
            );
        }
        db.add_tag(ids[0], "trip").unwrap();
        let col = db.create_collection("C").unwrap();
        db.add_to_collection(col.id, ids[0]).unwrap();
        db.conn()
            .execute(
                "UPDATE screenshots SET ocr_status = 'done', note = 'n1' WHERE id = ?1",
                rusqlite::params![ids[0]],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO ocr_text (screenshot_id, text) VALUES (?1, 'hello world')",
                rusqlite::params![ids[0]],
            )
            .unwrap();
        (dir, db, ids)
    }

    #[test]
    fn cleanup_trash_records_memories_when_enabled() {
        let (_dir, db, ids) = memory_fixture();
        let s = cleanup_trash(&db, &ids, true, true).unwrap();
        assert_eq!(s.trashed, 2);
        assert_eq!(s.memories_retained, 2);
        assert!(s.failed.is_empty());

        let (total, rows) = db.list_deleted_memories("", 10, 0).unwrap();
        assert_eq!(total, 2);
        let first = rows.iter().find(|r| r.filename == "m1.png").unwrap();
        assert_eq!(first.size, 4800);
        assert_eq!(first.ocr_text, "hello world");
        assert_eq!(first.note, "n1");
        assert!(first.keep_thumbnail);
        assert!(first.tags.contains("trip"));
        assert!(first.collections.contains('C'));
        // Records survive as searchable metadata, not backups.
        assert!(!first.deleted_at.is_empty());
    }

    #[test]
    fn cleanup_trash_skips_memories_when_disabled_or_already_gone() {
        let (_dir, db, ids) = memory_fixture();
        let s = cleanup_trash(&db, &[ids[0]], false, false).unwrap();
        assert_eq!(s.trashed, 1);
        assert_eq!(s.memories_retained, 0);
        let (total, _) = db.list_deleted_memories("", 10, 0).unwrap();
        assert_eq!(total, 0);

        // Unknown id fails; already-gone file records nothing.
        let missing = db
            .insert_screenshot(&NewScreenshot {
                path: "/tmp/shotmemory-gone/z.png".into(),
                filename: "z.png".into(),
                ..Default::default()
            })
            .unwrap();
        let s = cleanup_trash(&db, &[missing, 424242], true, false).unwrap();
        assert_eq!(s.already_missing, 1);
        assert_eq!(s.failed.len(), 1);
        assert_eq!(s.memories_retained, 0);
    }

    #[test]
    fn deleted_memory_list_filter_and_clear() {
        let (_dir, db, ids) = memory_fixture();
        cleanup_trash(&db, &ids, true, false).unwrap();
        let (total, rows) = db.list_deleted_memories("hello", 10, 0).unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].filename, "m1.png");
        let (total, _) = db.list_deleted_memories("no-such-thing", 10, 0).unwrap();
        assert_eq!(total, 0);
        assert!(db.delete_deleted_memory(rows[0].id).unwrap());
        assert!(!db.delete_deleted_memory(999999).unwrap());
        assert_eq!(db.clear_deleted_memories().unwrap(), 1);
        let (total, _) = db.list_deleted_memories("", 10, 0).unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn selection_bytes_sums_stored_sizes() {
        let (_dir, db, ids) = memory_fixture();
        assert_eq!(selection_bytes(&db, &ids).unwrap(), 9600);
        assert_eq!(selection_bytes(&db, &[]).unwrap(), 0);
        assert_eq!(selection_bytes(&db, &[ids[0], 424242]).unwrap(), 4800);
    }

    #[test]
    fn migration_v4_creates_deleted_memories_on_fresh_db() {
        let db = Database::open_in_memory().unwrap();
        let (total, _) = db.list_deleted_memories("", 10, 0).unwrap();
        assert_eq!(total, 0);
    }
}
