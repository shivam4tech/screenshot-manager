//! Read-only storage analysis for the Cleanup dashboard (Sprint 1).
//!
//! Everything here answers two questions from indexed metadata alone:
//! *WHAT can I review?* and *HOW MUCH space could review recover?*
//! No filesystem writes, no deletions, no renames — this module must never
//! touch files. All queries consider `available` records only, so missing
//! files can never inflate reclaimable numbers, and category totals are
//! reported per category (a file in several categories is never double
//! counted in the single "exact duplicate savings" headline).

use rusqlite::params;
use serde::Serialize;

use crate::db::Database;
use crate::error::{CoreError, CoreResult};
use crate::insights::{detect_bursts, similar_groups};

/// One screenshot row for cleanup review, including its stored byte size.
#[derive(Debug, Clone, Serialize)]
pub struct CleanupItem {
    pub id: i64,
    pub path: String,
    pub filename: String,
    pub size: i64,
    pub created_ts: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub format: Option<String>,
    pub content_hash: Option<String>,
    pub starred: bool,
}

fn map_cleanup_item(row: &rusqlite::Row) -> rusqlite::Result<CleanupItem> {
    Ok(CleanupItem {
        id: row.get(0)?,
        path: row.get(1)?,
        filename: row.get(2)?,
        size: row.get(3)?,
        created_ts: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        format: row.get(7)?,
        content_hash: row.get(8)?,
        starred: row.get::<_, i64>(9)? != 0,
    })
}

const ITEM_COLS: &str = "id, path, filename, size, created_ts, width, height, format, content_hash, starred";

/// One duplicate group with its reclaimable bytes. For byte-identical files
/// every member has the same size, so keeping one copy reclaims
/// `(members - 1) * size`. Near-duplicate groups report the same figure but
/// callers must label it a *candidate* upper bound, never a promise.
#[derive(Debug, Clone, Serialize)]
pub struct CleanupGroup {
    pub kind: String,
    pub key: String,
    pub items: Vec<CleanupItem>,
    pub reclaim_bytes: i64,
    /// Id of the recommended keeper (newest available member, if any).
    pub keep_id: Option<i64>,
    /// True when members carry differing tags/collections/notes/stars, so
    /// removing one may discard metadata the others lack.
    pub metadata_warning: bool,
}

/// Aggregate counts for one cleanup category.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CategoryStat {
    pub count: i64,
    pub bytes: i64,
}

/// Whole-library cleanup overview in a single round-trip.
#[derive(Debug, Clone, Serialize)]
pub struct CleanupOverview {
    pub total_count: i64,
    pub total_bytes: i64,
    pub exact_groups: i64,
    pub exact_count: i64,
    /// Bytes truly recoverable: redundant byte-identical copies only.
    pub exact_reclaim_bytes: i64,
    pub near_groups: i64,
    pub near_count: i64,
    /// Upper-bound candidate size; reviewing may keep everything.
    pub near_candidate_bytes: i64,
    pub burst_groups: i64,
    pub burst_count: i64,
    pub burst_bytes: i64,
    pub old: CategoryStat,
    pub old_age_days: i64,
    pub large: CategoryStat,
    pub large_min_bytes: i64,
    pub notext: CategoryStat,
    /// UTC timestamp of this analysis, for "last analyzed" display.
    pub analyzed_at: String,
}

/// Reviewable item categories (bursts drill down via the Bursts view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupCategory {
    Old,
    Large,
    NoText,
}

impl CleanupCategory {
    fn parse(s: &str) -> CoreResult<Self> {
        match s {
            "old" => Ok(CleanupCategory::Old),
            "large" => Ok(CleanupCategory::Large),
            "notext" => Ok(CleanupCategory::NoText),
            _ => Err(CoreError::other(
                "unknown cleanup category (expected old, large, or notext)",
            )),
        }
    }
}

/// Sort orders for item review. Allowlisted — never interpolated raw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupSort {
    Oldest,
    Newest,
    Largest,
}

impl CleanupSort {
    fn parse(s: &str) -> CoreResult<Self> {
        match s {
            "oldest" => Ok(CleanupSort::Oldest),
            "newest" => Ok(CleanupSort::Newest),
            "largest" => Ok(CleanupSort::Largest),
            _ => Err(CoreError::other(
                "unknown cleanup sort (expected oldest, newest, or largest)",
            )),
        }
    }

    fn order_by(self) -> &'static str {
        match self {
            CleanupSort::Oldest => "COALESCE(created_ts, modified_ts) ASC, id ASC",
            CleanupSort::Newest => "COALESCE(created_ts, modified_ts) DESC, id DESC",
            CleanupSort::Largest => "size DESC, id DESC",
        }
    }
}

/// Allowed age thresholds in days (old-screenshots category).
pub const AGE_OPTIONS: [i64; 5] = [30, 90, 180, 365, 730];
/// Allowed size thresholds in bytes (large-screenshots category).
pub const SIZE_OPTIONS: [i64; 3] = [2_000_000, 5_000_000, 10_000_000];
pub const DEFAULT_AGE_DAYS: i64 = 365;
pub const DEFAULT_MIN_BYTES: i64 = 5_000_000;
/// Similarity threshold matching the Duplicates page default.
pub const DEFAULT_SIMILAR_DISTANCE: u32 = 8;
/// Burst gap matching the Bursts page default (30 minutes).
pub const DEFAULT_BURST_GAP_SECS: i64 = 1800;

fn valid_age(age_days: i64) -> CoreResult<i64> {
    if AGE_OPTIONS.contains(&age_days) {
        Ok(age_days)
    } else {
        Err(CoreError::other(
            "unknown age threshold (expected 30, 90, 180, 365, or 730 days)",
        ))
    }
}

fn valid_size(size_bytes: i64) -> CoreResult<i64> {
    if SIZE_OPTIONS.contains(&size_bytes) {
        Ok(size_bytes)
    } else {
        Err(CoreError::other(
            "unknown size threshold (expected 2000000, 5000000, or 10000000 bytes)",
        ))
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Byte-identical groups with sizes, newest member recommended as keeper.
/// Items arrive newest-first so `keep_id` is simply the first member.
fn exact_groups_with_sizes(db: &Database) -> CoreResult<Vec<CleanupGroup>> {
    let hashes: Vec<String> = db
        .conn()
        .prepare(
            "SELECT content_hash FROM screenshots
              WHERE status = 'available' AND content_hash IS NOT NULL
              GROUP BY content_hash HAVING COUNT(*) > 1
              ORDER BY COUNT(*) DESC",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut groups = Vec::with_capacity(hashes.len());
    for h in hashes {
        let mut stmt = db.conn().prepare(&format!(
            "SELECT {ITEM_COLS} FROM screenshots
              WHERE status = 'available' AND content_hash = ?1
              ORDER BY COALESCE(created_ts, modified_ts) DESC, id DESC",
        ))?;
        let items = stmt
            .query_map(params![h], map_cleanup_item)?
            .collect::<Result<Vec<_>, _>>()?;
        if items.len() < 2 {
            continue;
        }
        let size = items[0].size;
        let keep_id = items.first().map(|r| r.id);
        groups.push(CleanupGroup {
            kind: "exact".into(),
            key: h,
            reclaim_bytes: (items.len() as i64 - 1) * size,
            metadata_warning: metadata_differs(db, &items)?,
            items,
            keep_id,
        });
    }
    Ok(groups)
}

/// True when group members carry differing user metadata (tags, collection
/// membership, notes, or star state), so deleting one may lose something.
fn metadata_differs(db: &Database, items: &[CleanupItem]) -> CoreResult<bool> {
    if items.len() < 2 {
        return Ok(false);
    }
    let ids: Vec<i64> = items.iter().map(|r| r.id).collect();
    let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
    let list = placeholders.join(",");
    let sql = format!(
        "SELECT COUNT(DISTINCT
            starred || '|' || read_later || '|' || note || '|' ||
            (SELECT GROUP_CONCAT(tag_id, ',') FROM
                (SELECT tag_id FROM screenshot_tags WHERE screenshot_id = s.id ORDER BY tag_id)) || '|' ||
            (SELECT GROUP_CONCAT(collection_id, ',') FROM
                (SELECT collection_id FROM collection_items WHERE screenshot_id = s.id ORDER BY collection_id))
        ) FROM screenshots s WHERE s.id IN ({list})"
    );
    let mut stmt = db.conn().prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let distinct: i64 = stmt.query_row(params.as_slice(), |r| r.get(0))?;
    Ok(distinct > 1)
}

/// Sizes for a set of ids (used to total near-duplicate candidates).
fn sizes_for_ids(db: &Database, ids: &[i64]) -> CoreResult<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT size FROM screenshots WHERE id IN ({})",
        placeholders.join(",")
    );
    let mut stmt = db.conn().prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let rows: Vec<i64> = stmt
        .query_map(params.as_slice(), |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Total count + bytes of the available library.
fn library_totals(db: &Database) -> CoreResult<(i64, i64)> {
    db.conn()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM screenshots WHERE status = 'available'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.into())
}

/// Aggregate count + bytes for old screenshots.
fn old_stat(db: &Database, age_days: i64) -> CoreResult<CategoryStat> {
    let cutoff = now_secs() - age_days * 86_400;
    let (count, bytes): (i64, i64) = db
        .conn()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM screenshots
              WHERE status = 'available'
                AND COALESCE(created_ts, modified_ts) IS NOT NULL
                AND COALESCE(created_ts, modified_ts) < ?1",
            params![cutoff],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
    Ok(CategoryStat { count, bytes })
}

/// Aggregate count + bytes for large screenshots.
fn large_stat(db: &Database, min_bytes: i64) -> CoreResult<CategoryStat> {
    let (count, bytes): (i64, i64) = db
        .conn()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM screenshots
              WHERE status = 'available' AND size >= ?1",
            params![min_bytes],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
    Ok(CategoryStat { count, bytes })
}

/// Aggregate count + bytes for screenshots with no detected text
/// (OCR never completed, or completed empty).
fn notext_stat(db: &Database) -> CoreResult<CategoryStat> {
    let (count, bytes): (i64, i64) = db
        .conn()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(s.size), 0) FROM screenshots s
              WHERE s.status = 'available'
                AND (s.ocr_status <> 'done'
                     OR TRIM(COALESCE((SELECT o.text FROM ocr_text o WHERE o.screenshot_id = s.id), '')) = '')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
    Ok(CategoryStat { count, bytes })
}

/// Full cleanup overview in a handful of indexed queries.
pub fn cleanup_overview(db: &Database) -> CoreResult<CleanupOverview> {    let (total_count, total_bytes) = library_totals(db)?;

    let exact_groups = exact_groups_with_sizes(db)?;
    let exact_count: i64 = exact_groups.iter().map(|g| g.items.len() as i64).sum();
    let exact_reclaim_bytes: i64 = exact_groups.iter().map(|g| g.reclaim_bytes).sum();

    let near_groups = similar_groups(db, DEFAULT_SIMILAR_DISTANCE)?;
    let near_count: i64 = near_groups.iter().map(|g| g.items.len() as i64).sum();
    let near_ids: Vec<i64> = near_groups
        .iter()
        .flat_map(|g| g.items.iter().map(|r| r.id))
        .collect();
    let near_candidate_bytes: i64 = sizes_for_ids(db, &near_ids)?.iter().sum();

    let bursts = detect_bursts(db, DEFAULT_BURST_GAP_SECS)?;
    let mut burst_count = 0i64;
    let mut burst_bytes = 0i64;
    for b in &bursts {
        let (count, bytes): (i64, i64) = db
            .conn()
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM screenshots
                  WHERE status = 'available'
                    AND COALESCE(created_ts, modified_ts) BETWEEN ?1 AND ?2",
                params![b.start_ts, b.end_ts],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
        burst_count += count;
        burst_bytes += bytes;
    }

    let old = old_stat(db, DEFAULT_AGE_DAYS)?;
    let large = large_stat(db, DEFAULT_MIN_BYTES)?;
    let notext = notext_stat(db)?;
    let analyzed_at: String = db
        .conn()
        .query_row("SELECT datetime('now')", [], |r| r.get(0))?;

    Ok(CleanupOverview {
        total_count,
        total_bytes,
        exact_groups: exact_groups.len() as i64,
        exact_count,
        exact_reclaim_bytes,
        near_groups: near_groups.len() as i64,
        near_count,
        near_candidate_bytes,
        burst_groups: bursts.len() as i64,
        burst_count,
        burst_bytes,
        old,
        old_age_days: DEFAULT_AGE_DAYS,
        large,
        large_min_bytes: DEFAULT_MIN_BYTES,
        notext,
        analyzed_at,
    })
}

/// Paged review items for the old / large / no-text categories.
pub struct CleanupPage {
    pub total: i64,
    pub bytes: i64,
    pub rows: Vec<CleanupItem>,
}

impl serde::Serialize for CleanupPage {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("CleanupPage", 3)?;
        st.serialize_field("total", &self.total)?;
        st.serialize_field("bytes", &self.bytes)?;
        st.serialize_field("rows", &self.rows)?;
        st.end()
    }
}

pub fn cleanup_items(
    db: &Database,
    category_raw: &str,
    age_days_raw: i64,
    size_bytes_raw: i64,
    sort_raw: &str,
    limit_raw: i64,
    offset_raw: i64,
) -> CoreResult<CleanupPage> {
    let category = CleanupCategory::parse(category_raw)?;
    let sort = CleanupSort::parse(sort_raw)?;
    let limit = limit_raw.clamp(1, 200);
    let offset = offset_raw.max(0);

    let where_clause: String;
    let mut args: Vec<i64> = Vec::new();
    match category {
        CleanupCategory::Old => {
            let age_days = valid_age(age_days_raw)?;
            where_clause = "s.status = 'available'
                AND COALESCE(s.created_ts, s.modified_ts) IS NOT NULL
                AND COALESCE(s.created_ts, s.modified_ts) < ?1"
                .into();
            args.push(now_secs() - age_days * 86_400);
        }
        CleanupCategory::Large => {
            let min_bytes = valid_size(size_bytes_raw)?;
            where_clause = "s.status = 'available' AND s.size >= ?1".into();
            args.push(min_bytes);
        }
        CleanupCategory::NoText => {
            where_clause = "s.status = 'available'
                AND (s.ocr_status <> 'done'
                     OR TRIM(COALESCE((SELECT o.text FROM ocr_text o WHERE o.screenshot_id = s.id), '')) = '')"
                .into();
        }
    }

    let (total, bytes): (i64, i64) = db
        .conn()
        .query_row(
            &format!("SELECT COUNT(*), COALESCE(SUM(s.size), 0) FROM screenshots s WHERE {where_clause}"),
            rusqlite::params_from_iter(args.iter()),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
    let cols = ITEM_COLS
        .split(", ")
        .map(|c| format!("s.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = db.conn().prepare(&format!(
        "SELECT {cols} FROM screenshots s WHERE {where_clause} ORDER BY {} LIMIT ?{} OFFSET ?{}",
        sort.order_by(),
        args.len() + 1,
        args.len() + 2,
    ))?;
    let mut all_args: Vec<i64> = args;
    all_args.push(limit);
    all_args.push(offset);
    let rows = stmt
        .query_map(rusqlite::params_from_iter(all_args.iter()), map_cleanup_item)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CleanupPage { total, bytes, rows })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScreenshot;

    /// Fixture: two identical small files, a 5-member identical group, one
    /// old large file, one recent tiny file, one missing record, one OCR-less
    /// file. Sizes chosen so every expected total is hand-computable.
    fn fixture() -> Database {
        let db = Database::open_in_memory().unwrap();
        let now = now_secs();
        let day = 86_400;
        // (name, size, age_days, content_hash, phash, status, ocr_done_with_text)
        let rows = [
            ("dup_a.png", 1000_i64, 10_i64, "h1", 0x1111_1111_1111_1111u64, "available", true),
            ("dup_b.png", 1000, 9, "h1", 0x1111_1111_1111_1111u64, "available", true),
            ("g1.png", 500, 400, "h2", 0x2222_2222_2222_2222u64, "available", true),
            ("g2.png", 500, 400, "h2", 0x2222_2222_2222_2222u64, "available", true),
            ("g3.png", 500, 400, "h2", 0x2222_2222_2222_2222u64, "available", true),
            ("g4.png", 500, 400, "h2", 0x2222_2222_2222_2222u64, "available", true),
            ("g5.png", 500, 400, "h2", 0x2222_2222_2222_2222u64, "available", true),
            ("old_big.png", 6_000_000, 500, "h3", 0x3333_3333_3333_3333u64, "available", true),
            ("new_tiny.png", 100, 1, "h4", 0x4444_4444_4444_4444u64, "available", false),
            ("gone.png", 1000, 400, "h1", 0x5555_5555_5555_5555u64, "missing", true),
        ];
        for (i, (name, size, age, hash, ph, status, ocr)) in rows.iter().enumerate() {
            let ts = now - age * day;
            let id = db
                .insert_screenshot(&NewScreenshot {
                    path: format!("/tmp/{name}"),
                    filename: (*name).into(),
                    size: *size,
                    created_ts: Some(ts),
                    modified_ts: Some(ts),
                    content_hash: Some((*hash).into()),
                    phash: Some(crate::hashing::phash_to_hex(*ph)),
                    ..Default::default()
                })
                .unwrap();
            if *status != "available" {
                db.conn()
                    .execute("UPDATE screenshots SET status = 'missing' WHERE id = ?1", params![id])
                    .unwrap();
            }
            if *ocr {
                db.conn()
                    .execute(
                        "UPDATE screenshots SET ocr_status = 'done' WHERE id = ?1",
                        params![id],
                    )
                    .unwrap();
                db.conn()
                    .execute(
                        "INSERT INTO ocr_text (screenshot_id, text) VALUES (?1, 'some words here')",
                        params![id],
                    )
                    .unwrap();
            }
            let _ = i;
        }
        db
    }

    #[test]
    fn overview_counts_and_reclaim_are_exact() {
        let db = fixture();
        let o = cleanup_overview(&db).unwrap();
        // Available: everything but gone.png → 9 files.
        assert_eq!(o.total_count, 9);
        assert_eq!(
            o.total_bytes,
            1000 + 1000 + 500 * 5 + 6_000_000 + 100
        );
        // Exact: {dup_a,dup_b} reclaim 1000; {g1..g5} reclaim 4*500=2000.
        assert_eq!(o.exact_groups, 2);
        assert_eq!(o.exact_count, 7);
        assert_eq!(o.exact_reclaim_bytes, 3000);
        // Missing gone.png (same hash h1, size 1000) must not inflate it.
        // Old (>365d): g1..g5 + old_big → 6 files.
        assert_eq!(o.old.count, 6);
        assert_eq!(o.old.bytes, 500 * 5 + 6_000_000);
        // Large (>=5MB): old_big only.
        assert_eq!(o.large.count, 1);
        assert_eq!(o.large.bytes, 6_000_000);
        // No text: only new_tiny (ocr never done).
        assert_eq!(o.notext.count, 1);
    }

    #[test]
    fn same_file_in_many_categories_is_not_double_counted_in_reclaim() {
        let db = fixture();
        let o = cleanup_overview(&db).unwrap();
        // g1..g5 sit in exact + old; the headline counts each copy once.
        assert_eq!(o.exact_reclaim_bytes, 3000);
        assert!(o.old.bytes >= 6_000_000);
    }

    #[test]
    fn items_paging_sorts_and_params_are_validated() {
        let db = fixture();
        let page = cleanup_items(&db, "old", 365, 0, "oldest", 10, 0).unwrap();
        assert_eq!(page.total, 6);
        assert!(page.rows.windows(2).all(|w| {
            w[0].created_ts.unwrap_or(0) <= w[1].created_ts.unwrap_or(0)
        }));
        let big = cleanup_items(&db, "large", 0, 5_000_000, "largest", 10, 0).unwrap();
        assert_eq!(big.total, 1);
        assert_eq!(big.rows[0].filename, "old_big.png");
        assert!(cleanup_items(&db, "bogus", 0, 0, "oldest", 10, 0).is_err());
        assert!(cleanup_items(&db, "old", 7, 0, "oldest", 10, 0).is_err());
        assert!(cleanup_items(&db, "large", 0, 123, "largest", 10, 0).is_err());
        assert!(cleanup_items(&db, "old", 365, 0, "random", 10, 0).is_err());
        let p1 = cleanup_items(&db, "old", 365, 0, "oldest", 2, 0).unwrap();
        let p2 = cleanup_items(&db, "old", 365, 0, "oldest", 2, 2).unwrap();
        assert_eq!(p1.rows.len(), 2);
        assert_eq!(p2.rows.len(), 2);
        assert_ne!(p1.rows[0].id, p2.rows[0].id);
    }

    #[test]
    fn notext_lists_only_textless_available_files() {
        let db = fixture();
        let page = cleanup_items(&db, "notext", 0, 0, "newest", 10, 0).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].filename, "new_tiny.png");
    }

    #[test]
    fn empty_library_gives_zero_overview() {
        let db = Database::open_in_memory().unwrap();
        let o = cleanup_overview(&db).unwrap();
        assert_eq!(o.total_count, 0);
        assert_eq!(o.exact_reclaim_bytes, 0);
        assert_eq!(o.total_bytes, 0);
    }
}
