//! Timeline and duplicate-group queries over the indexed library.
//!
//! Read-only views built on top of [`Database`]: month/day buckets for the
//! timeline browser, exact-duplicate groups (shared SHA-256 content hash),
//! and near-duplicate groups (dHash Hamming distance within a threshold).
//! No file mutation happens here — groups are for review and bulk
//! organization (tag, star, collect), never deletion.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::{map_screenshot_row, Database, ScreenshotRow};
use crate::error::{CoreError, CoreResult};

/// One month bucket in the timeline browser.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineMonth {
    pub year: i32,
    pub month: u32,
    /// "YYYY-MM" key for round-tripping into [`timeline_days`].
    pub key: String,
    pub count: i64,
}

/// One day bucket within a month.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineDay {
    /// "YYYY-MM-DD" key for round-tripping into [`timeline_items`].
    pub date: String,
    pub count: i64,
}

/// A burst: screenshots captured close together in time, with the theme
/// signals (dominant app/category/tags) that describe what the burst is
/// about. The fast path to "that afternoon I researched X".
#[derive(Debug, Clone, Serialize)]
pub struct Burst {
    /// "startTs-endTs" key for round-tripping into [`burst_items`].
    pub key: String,
    pub start_ts: i64,
    pub end_ts: i64,
    pub count: i64,
    pub top_category: Option<String>,
    pub top_app: Option<String>,
    pub top_tags: Vec<String>,
    /// Content hashes of up to 4 newest members (thumbnail strip previews).
    pub preview_hashes: Vec<Option<String>>,
}

/// Group screenshots into bursts: a new burst starts when the gap between
/// consecutive captures exceeds `max_gap_secs` (clamped to 1 min – 1 day,
/// default 30 min). Only groups of 2+ are returned, newest first.
pub fn detect_bursts(db: &Database, max_gap_secs: i64) -> CoreResult<Vec<Burst>> {
    let gap = max_gap_secs.clamp(60, 86_400);
    let mut stmt = db.conn().prepare(
        "SELECT id, COALESCE(created_ts, modified_ts) AS ts
         FROM screenshots
         WHERE COALESCE(created_ts, modified_ts) IS NOT NULL
         ORDER BY ts",
    )?;
    let ordered = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    // Split into runs separated by gaps.
    let mut runs: Vec<Vec<(i64, i64)>> = Vec::new();
    for (id, ts) in ordered {
        let extend = runs
            .last()
            .and_then(|run| run.last())
            .map(|&(_, prev_ts)| ts - prev_ts <= gap)
            .unwrap_or(false);
        if extend {
            runs.last_mut().unwrap().push((id, ts));
        } else {
            runs.push(vec![(id, ts)]);
        }
    }

    let mut bursts = Vec::new();
    for run in runs {
        if run.len() < 2 {
            continue;
        }
        let start_ts = run.first().unwrap().1;
        let end_ts = run.last().unwrap().1;
        bursts.push(Burst {
            key: format!("{start_ts}-{end_ts}"),
            start_ts,
            end_ts,
            count: run.len() as i64,
            top_category: burst_mode(db, start_ts, end_ts, "category")?,
            top_app: burst_mode(db, start_ts, end_ts, "app_name")?,
            top_tags: burst_top_tags(db, start_ts, end_ts)?,
            preview_hashes: burst_previews(db, start_ts, end_ts)?,
        });
    }
    bursts.sort_by(|a, b| b.start_ts.cmp(&a.start_ts));
    Ok(bursts)
}

/// Most common non-null value of a column within a time window.
fn burst_mode(
    db: &Database,
    start_ts: i64,
    end_ts: i64,
    column: &str,
) -> CoreResult<Option<String>> {
    // Column is caller-controlled from a fixed set, never user input.
    let sql = format!(
        "SELECT {column} FROM screenshots
         WHERE COALESCE(created_ts, modified_ts) BETWEEN ?1 AND ?2
           AND {column} IS NOT NULL
         GROUP BY {column} ORDER BY COUNT(*) DESC LIMIT 1"
    );
    Ok(db
        .conn()
        .query_row(&sql, params![start_ts, end_ts], |r| r.get(0))
        .optional()?)
}

/// Up to 3 most-used tags within a time window.
fn burst_top_tags(db: &Database, start_ts: i64, end_ts: i64) -> CoreResult<Vec<String>> {
    let mut stmt = db.conn().prepare(
        "SELECT t.name, COUNT(*) AS n FROM tags t
         JOIN screenshot_tags st ON st.tag_id = t.id
         JOIN screenshots s ON s.id = st.screenshot_id
         WHERE COALESCE(s.created_ts, s.modified_ts) BETWEEN ?1 AND ?2
         GROUP BY t.id ORDER BY n DESC, t.name LIMIT 3",
    )?;
    let tags = stmt
        .query_map(params![start_ts, end_ts], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tags)
}

/// Content hashes of the 4 newest members (for preview strips).
fn burst_previews(
    db: &Database,
    start_ts: i64,
    end_ts: i64,
) -> CoreResult<Vec<Option<String>>> {
    let mut stmt = db.conn().prepare(
        "SELECT content_hash FROM screenshots
         WHERE COALESCE(created_ts, modified_ts) BETWEEN ?1 AND ?2
         ORDER BY COALESCE(created_ts, modified_ts) DESC, id DESC LIMIT 4",
    )?;
    let hashes = stmt
        .query_map(params![start_ts, end_ts], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(hashes)
}

/// Items captured in [start_ts, end_ts], newest first (burst drill-down).
pub fn burst_items(
    db: &Database,
    start_ts: i64,
    end_ts: i64,
    limit: i64,
    offset: i64,
) -> CoreResult<Vec<ScreenshotRow>> {
    if start_ts > end_ts || start_ts < 0 {
        return Err(CoreError::other("invalid burst range"));
    }
    let mut stmt = db.conn().prepare(
        "SELECT id, path, filename, created_ts, width, height, format,
                status, ocr_status, content_hash, phash, starred
         FROM screenshots
         WHERE COALESCE(created_ts, modified_ts) BETWEEN ?1 AND ?2
         ORDER BY COALESCE(created_ts, modified_ts) DESC, id DESC
         LIMIT ?3 OFFSET ?4",
    )?;
    let rows = stmt
        .query_map(
            params![start_ts, end_ts, limit, offset],
            map_screenshot_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every id in a burst window, newest first (select-all for bursts).
pub fn burst_range_ids(
    db: &Database,
    start_ts: i64,
    end_ts: i64,
) -> CoreResult<Vec<i64>> {
    if start_ts > end_ts || start_ts < 0 {
        return Err(CoreError::other("invalid burst range"));
    }
    let mut stmt = db.conn().prepare(
        "SELECT id FROM screenshots
         WHERE COALESCE(created_ts, modified_ts) BETWEEN ?1 AND ?2
         ORDER BY COALESCE(created_ts, modified_ts) DESC, id DESC",
    )?;
    let ids = stmt
        .query_map(params![start_ts, end_ts], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Every id captured on one local day, newest first (select-all for days).
pub fn timeline_item_ids(db: &Database, date: &str) -> CoreResult<Vec<i64>> {
    if !valid_day(date) {
        return Err(CoreError::other("date must be YYYY-MM-DD"));
    }
    let mut stmt = db.conn().prepare(
        "SELECT id FROM screenshots
         WHERE date(datetime(COALESCE(created_ts, modified_ts), 'unixepoch', 'localtime')) = ?1
         ORDER BY COALESCE(created_ts, modified_ts) DESC, id DESC",
    )?;
    let ids = stmt
        .query_map(params![date], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// A group of screenshots that are exact or near duplicates of each other.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroup {
    /// "exact" (shared content hash) or "similar" (perceptual hash cluster).
    pub kind: String,
    /// Group identity: the content hash, or the representative phash hex.
    pub key: String,
    pub items: Vec<ScreenshotRow>,
}

/// Months that contain screenshots, newest first. Dates use local time so
/// buckets match what the user saw on their own clock.
pub fn timeline_months(db: &Database) -> CoreResult<Vec<TimelineMonth>> {
    let mut stmt = db.conn().prepare(
        "SELECT CAST(strftime('%Y', datetime(COALESCE(created_ts, modified_ts), 'unixepoch', 'localtime')) AS INTEGER),
                CAST(strftime('%m', datetime(COALESCE(created_ts, modified_ts), 'unixepoch', 'localtime')) AS INTEGER),
                COUNT(*)
         FROM screenshots
         WHERE COALESCE(created_ts, modified_ts) IS NOT NULL
         GROUP BY 1, 2
         ORDER BY 1 DESC, 2 DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let year: i32 = r.get(0)?;
            let month: u32 = r.get(1)?;
            let count: i64 = r.get(2)?;
            Ok(TimelineMonth {
                year,
                month,
                key: format!("{year:04}-{month:02}"),
                count,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Day buckets within one month (1-12), newest first.
pub fn timeline_days(db: &Database, year: i32, month: u32) -> CoreResult<Vec<TimelineDay>> {
    if !(1..=12).contains(&month) || year < 1970 || year > 9999 {
        return Err(CoreError::other("month must be 1-12 and year 1970-9999"));
    }
    let mut stmt = db.conn().prepare(
        "SELECT date(datetime(COALESCE(created_ts, modified_ts), 'unixepoch', 'localtime')) AS day,
                COUNT(*)
         FROM screenshots
         WHERE COALESCE(created_ts, modified_ts) IS NOT NULL
           AND strftime('%Y-%m', datetime(COALESCE(created_ts, modified_ts), 'unixepoch', 'localtime'))
               = printf('%04d-%02d', ?1, ?2)
         GROUP BY day
         ORDER BY day DESC",
    )?;
    let rows = stmt
        .query_map(params![year, month], |r| {
            Ok(TimelineDay {
                date: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Items captured on one local day ("YYYY-MM-DD"), newest first.
pub fn timeline_items(
    db: &Database,
    date: &str,
    limit: i64,
    offset: i64,
) -> CoreResult<Vec<ScreenshotRow>> {
    if !valid_day(date) {
        return Err(CoreError::other("date must be YYYY-MM-DD"));
    }
    let mut stmt = db.conn().prepare(
        "SELECT id, path, filename, created_ts, width, height, format,
                status, ocr_status, content_hash, phash, starred
         FROM screenshots
         WHERE date(datetime(COALESCE(created_ts, modified_ts), 'unixepoch', 'localtime')) = ?1
         ORDER BY COALESCE(created_ts, modified_ts) DESC, id DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt
        .query_map(params![date, limit, offset], map_screenshot_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn valid_day(date: &str) -> bool {
    if date.len() != 10 {
        return false;
    }
    let b = date.as_bytes();
    if b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    b.iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Groups of available screenshots sharing one content hash (byte-identical
/// files, possibly under different paths). Largest groups first.
pub fn exact_duplicate_groups(db: &Database) -> CoreResult<Vec<DuplicateGroup>> {
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
        let mut stmt = db.conn().prepare(
            "SELECT id, path, filename, created_ts, width, height, format,
                    status, ocr_status, content_hash, phash, starred
             FROM screenshots
             WHERE status = 'available' AND content_hash = ?1
             ORDER BY COALESCE(created_ts, modified_ts) DESC, id DESC",
        )?;
        let items = stmt
            .query_map(params![h], map_screenshot_row)?
            .collect::<Result<Vec<_>, _>>()?;
        groups.push(DuplicateGroup {
            kind: "exact".into(),
            key: h,
            items,
        });
    }
    Ok(groups)
}

/// Greedy perceptual clusters: available screenshots whose dHash is within
/// `max_distance` bits of the group representative join that group.
/// Single-pass, O(n * groups); fine for personal libraries. Groups of two
/// or more are returned, largest first. `max_distance` is clamped to 0-16.
pub fn similar_groups(db: &Database, max_distance: u32) -> CoreResult<Vec<DuplicateGroup>> {
    let max_distance = max_distance.min(16);
    let mut stmt = db.conn().prepare(
        "SELECT id, path, filename, created_ts, width, height, format,
                status, ocr_status, content_hash, phash, starred
         FROM screenshots
         WHERE status = 'available' AND phash IS NOT NULL
         ORDER BY id",
    )?;
    let rows = stmt
        .query_map([], map_screenshot_row)?
        .collect::<Result<Vec<ScreenshotRow>, _>>()?;

    // (representative phash, member rows)
    let mut clusters: Vec<(u64, Vec<ScreenshotRow>)> = Vec::new();
    for row in rows {
        let Some(phash_hex) = row.phash.clone() else {
            continue;
        };
        let Ok(phash) = crate::hashing::phash_from_hex(&phash_hex) else {
            continue;
        };
        let mut placed = false;
        for (rep, members) in clusters.iter_mut() {
            if crate::hashing::hamming_distance(*rep, phash) <= max_distance {
                members.push(row.clone());
                placed = true;
                break;
            }
        }
        if !placed {
            clusters.push((phash, vec![row]));
        }
    }
    let mut groups: Vec<DuplicateGroup> = clusters
        .into_iter()
        .filter(|(_, m)| m.len() > 1)
        .map(|(rep, mut members)| {
            members.sort_by(|a, b| {
                b.created_ts
                    .unwrap_or(0)
                    .cmp(&a.created_ts.unwrap_or(0))
                    .then(b.id.cmp(&a.id))
            });
            DuplicateGroup {
                kind: "similar".into(),
                key: crate::hashing::phash_to_hex(rep),
                items: members,
            }
        })
        .collect();
    groups.sort_by(|a, b| b.items.len().cmp(&a.items.len()));
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScreenshot;

    fn fixture() -> Database {
        let db = Database::open_in_memory().unwrap();
        // Two shots on 2026-09-01 (local midday UTC for determinism in most
        // zones; assertions below are zone-tolerant), one on 2026-09-03.
        let days = [
            ("a.png", 1_787_054_400_i64), // 2026-08-31 16:00 UTC
            ("b.png", 1_787_058_000_i64),
            ("c.png", 1_787_227_200_i64), // 2026-09-02 16:00 UTC
        ];
        for (i, (name, ts)) in days.iter().enumerate() {
            let id = db
                .insert_screenshot(&NewScreenshot {
                    path: format!("/tmp/{name}"),
                    filename: (*name).into(),
                    created_ts: Some(*ts),
                    modified_ts: Some(*ts),
                    content_hash: Some(format!("hash{i}")),
                    phash: Some(crate::hashing::phash_to_hex(0x0102030405060708 + i as u64)),
                    ..Default::default()
                })
                .unwrap();
            // a.png and b.png are byte-identical.
            if *name != "c.png" {
                db.conn()
                    .execute(
                        "UPDATE screenshots SET content_hash = 'samebytes' WHERE id = ?1",
                        params![id],
                    )
                    .unwrap();
            }
        }
        db
    }

    #[test]
    fn timeline_buckets_and_items_cover_everything() {
        let db = fixture();
        let months = timeline_months(&db).unwrap();
        assert!(!months.is_empty());
        let total: i64 = months.iter().map(|m| m.count).sum();
        assert_eq!(total, 3);

        // Drill into the month holding the most shots; its days must cover them.
        let top = months.iter().max_by_key(|m| m.count).unwrap();
        let days = timeline_days(&db, top.year, top.month).unwrap();
        assert!(!days.is_empty());
        let day_total: i64 = days.iter().map(|d| d.count).sum();
        assert!(day_total >= 1);

        let items = timeline_items(&db, &days[0].date, 10, 0).unwrap();
        assert_eq!(items.len() as i64, days[0].count);

        assert!(timeline_days(&db, 2026, 13).is_err());
        assert!(timeline_items(&db, "not-a-date", 10, 0).is_err());
    }

    #[test]
    fn exact_groups_find_byte_identical_pair() {
        let db = fixture();
        let groups = exact_duplicate_groups(&db).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, "exact");
        assert_eq!(groups[0].key, "samebytes");
        assert_eq!(groups[0].items.len(), 2);
    }

    #[test]
    fn similar_groups_cluster_close_hashes() {
        let db = fixture();
        // phashes differ by 1-2 bits → one cluster at threshold 8.
        let groups = similar_groups(&db, 8).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 3);
        // Threshold 0 splits them apart.
        assert!(similar_groups(&db, 0).unwrap().is_empty());
    }

    #[test]
    fn bursts_group_close_captures_with_themes() {
        let db = Database::open_in_memory().unwrap();
        // Cluster 1: three shots within 10 minutes, all code-flavoured.
        // Cluster 2: two shots an hour later. Loner: days later (no burst).
        let base = 1_787_000_000_i64;
        let shots = [
            ("a1.png", base, Some("chrome"), Some("code")),
            ("a2.png", base + 300, Some("chrome"), Some("code")),
            ("a3.png", base + 600, None, Some("code")),
            ("b1.png", base + 7200, Some("slack"), Some("communication")),
            ("b2.png", base + 7500, Some("slack"), None),
            ("loner.png", base + 500_000, None, None),
        ];
        for (i, (name, ts, app, cat)) in shots.iter().enumerate() {
            let id = db
                .insert_screenshot(&NewScreenshot {
                    path: format!("/tmp/{name}"),
                    filename: (*name).into(),
                    created_ts: Some(*ts),
                    modified_ts: Some(*ts),
                    content_hash: Some(format!("h{i}")),
                    ..Default::default()
                })
                .unwrap();
            if let Some(a) = app {
                db.conn()
                    .execute(
                        "UPDATE screenshots SET app_name = ?1 WHERE id = ?2",
                        params![a, id],
                    )
                    .unwrap();
            }
            if let Some(c) = cat {
                db.conn()
                    .execute(
                        "UPDATE screenshots SET category = ?1 WHERE id = ?2",
                        params![c, id],
                    )
                    .unwrap();
            }
        }

        let bursts = detect_bursts(&db, 1800).unwrap();
        assert_eq!(bursts.len(), 2, "two clusters, loner excluded");
        // Newest first.
        assert_eq!(bursts[0].count, 2);
        assert_eq!(bursts[0].top_app.as_deref(), Some("slack"));
        assert_eq!(bursts[1].count, 3);
        assert_eq!(bursts[1].top_app.as_deref(), Some("chrome"));
        assert_eq!(bursts[1].top_category.as_deref(), Some("code"));
        assert!(!bursts[1].preview_hashes.is_empty());

        let items = burst_items(&db, bursts[1].start_ts, bursts[1].end_ts, 10, 0).unwrap();
        assert_eq!(items.len(), 3);
        assert!(burst_items(&db, 5, 1, 10, 0).is_err());
    }
}
