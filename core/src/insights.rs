//! Timeline and duplicate-group queries over the indexed library.
//!
//! Read-only views built on top of [`Database`]: month/day buckets for the
//! timeline browser, exact-duplicate groups (shared SHA-256 content hash),
//! and near-duplicate groups (dHash Hamming distance within a threshold).
//! No file mutation happens here — groups are for review and bulk
//! organization (tag, star, collect), never deletion.

use rusqlite::params;
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
}
