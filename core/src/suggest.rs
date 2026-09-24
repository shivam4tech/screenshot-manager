//! Suggested labeling (Tier 0 + Tier 1): score unfiled screenshots against
//! existing collections/tags using only stored signals (app, category,
//! domain, tags, bursts, perceptual hashes), and cluster leftovers into
//! named collection proposals. No network, no models, no image decoding —
//! every input is already indexed. All suggestions require one user click;
//! accepts/dismisses are logged as future training labels.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::Database;
use crate::error::CoreResult;
use crate::hashing::{hamming_distance, phash_from_hex};
use crate::insights::detect_bursts;

const MIN_COLLECTION_SCORE: i64 = 4;
const MIN_TAG_SCORE: i64 = 3;
const MAX_TARGETS: usize = 3;
const MAX_PROPOSALS: usize = 8;
const MIN_PROPOSAL_SIZE: usize = 3;
const MAX_PROPOSAL_MEMBERS: usize = 200;
const MEMBER_SAMPLE_CAP: usize = 300;
const BURST_GAP_SECS: i64 = 1800;
const PHASH_DISTANCE: u32 = 8;

/// One scored suggestion target for a single screenshot.
#[derive(Debug, Clone, Serialize)]
pub struct ScoredTarget {
    /// "collection" or "tag".
    pub kind: String,
    pub id: Option<i64>,
    pub name: String,
    pub score: i64,
    pub reasons: Vec<String>,
}

/// Tier 0 suggestions for one screenshot.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ShotSuggestions {
    pub collections: Vec<ScoredTarget>,
    pub tags: Vec<ScoredTarget>,
}

/// One proposed new collection (Tier 1).
#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub key: String,
    pub name: String,
    pub reasons: Vec<String>,
    pub member_count: i64,
    pub member_ids: Vec<i64>,
    pub preview_hashes: Vec<Option<String>>,
}

/// Sidebar/banner overview: proposals plus Tier 0 coverage.
#[derive(Debug, Clone, Serialize)]
pub struct SuggestOverview {
    pub proposals: Vec<Proposal>,
    pub suggested_count: i64,
    pub first_suggested_id: Option<i64>,
}

#[derive(Debug, Clone, Default)]
struct ShotProfile {
    app: Option<String>,
    category: Option<String>,
    domain: Option<String>,
    tags: HashSet<String>,
    burst: Option<String>,
    phash: Option<u64>,
}

fn shot_profile(db: &Database, id: i64) -> CoreResult<Option<ShotProfile>> {
    let row: Option<(Option<String>, Option<String>, Option<String>, Option<String>, String)> = db
        .conn()
        .query_row(
            "SELECT app_name, category, website_domain, phash, status
             FROM screenshots WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((app, category, domain, phash_hex, status)) = row else {
        return Ok(None);
    };
    if status != "available" {
        return Ok(None);
    }
    let mut stmt = db.conn().prepare(
        "SELECT t.name FROM tags t
         JOIN screenshot_tags st ON st.tag_id = t.id
         WHERE st.screenshot_id = ?1",
    )?;
    let tags: HashSet<String> = stmt
        .query_map(params![id], |r| r.get(0))?
        .collect::<Result<HashSet<_>, _>>()?;
    let phash = phash_hex
        .as_deref()
        .and_then(|h| phash_from_hex(h).ok());
    Ok(Some(ShotProfile {
        app,
        category,
        domain,
        tags,
        burst: None,
        phash,
    }))
}

#[derive(Debug, Clone, Default)]
struct CollectionProfile {
    id: i64,
    name: String,
    top_app: Option<String>,
    top_category: Option<String>,
    top_domain: Option<String>,
    tags: HashSet<String>,
    member_ids: HashSet<i64>,
    burst_keys: HashSet<String>,
    phashes: Vec<u64>,
}

fn top_of(values: &[Option<String>]) -> Option<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for v in values.iter().flatten() {
        *counts.entry(v.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(s, _)| s.to_string())
}

fn collection_profiles(db: &Database) -> CoreResult<Vec<CollectionProfile>> {
    let cols: Vec<(i64, String)> = db
        .conn()
        .prepare("SELECT id, name FROM collections ORDER BY name")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::with_capacity(cols.len());
    for (id, name) in cols {
        let mut stmt = db.conn().prepare(
            "SELECT s.id, s.app_name, s.category, s.website_domain, s.phash
             FROM screenshots s
             JOIN collection_items ci ON ci.screenshot_id = s.id
             WHERE ci.collection_id = ?1 AND s.status = 'available'
             ORDER BY s.id LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![id, MEMBER_SAMPLE_CAP as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let member_ids: HashSet<i64> = rows.iter().map(|r| r.0).collect();
        let mut tags = HashSet::new();
        if !member_ids.is_empty() {
            let holders: Vec<String> = member_ids.iter().map(|_| "?".into()).collect();
            let mut tstmt = db.conn().prepare(&format!(
                "SELECT DISTINCT t.name FROM tags t
                 JOIN screenshot_tags st ON st.tag_id = t.id
                 WHERE st.screenshot_id IN ({})",
                holders.join(",")
            ))?;
            let params: Vec<&dyn rusqlite::ToSql> =
                member_ids.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
            let tag_rows: Vec<String> = tstmt
                .query_map(params.as_slice(), |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            tags = tag_rows.into_iter().collect();
        }
        let phashes = rows
            .iter()
            .filter_map(|r| r.4.as_deref())
            .filter_map(|h| phash_from_hex(h).ok())
            .collect();
        out.push(CollectionProfile {
            id,
            name,
            top_app: top_of(&rows.iter().map(|r| r.1.clone()).collect::<Vec<_>>()),
            top_category: top_of(&rows.iter().map(|r| r.2.clone()).collect::<Vec<_>>()),
            top_domain: top_of(&rows.iter().map(|r| r.3.clone()).collect::<Vec<_>>()),
            tags,
            member_ids,
            burst_keys: HashSet::new(),
            phashes,
        });
    }
    // Annotate burst membership once (shared across collections).
    let burst_of = burst_membership(db)?;
    for p in out.iter_mut() {
        for id in &p.member_ids {
            if let Some(k) = burst_of.get(id) {
                p.burst_keys.insert(k.clone());
            }
        }
    }
    Ok(out)
}

/// Map each available screenshot id to its burst key ("start-end").
fn burst_membership(db: &Database) -> CoreResult<HashMap<i64, String>> {
    let mut map = HashMap::new();
    for b in detect_bursts(db, BURST_GAP_SECS)? {
        let mut stmt = db.conn().prepare(
            "SELECT id FROM screenshots
             WHERE status = 'available'
               AND COALESCE(created_ts, modified_ts) BETWEEN ?1 AND ?2",
        )?;
        let ids = stmt
            .query_map(params![b.start_ts, b.end_ts], |r| r.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        for id in ids {
            map.insert(id, b.key.clone());
        }
    }
    Ok(map)
}

fn member_collections(db: &Database, shot_id: i64) -> CoreResult<HashSet<i64>> {
    let mut stmt = db.conn().prepare(
        "SELECT collection_id FROM collection_items WHERE screenshot_id = ?1",
    )?;
    let rows: Vec<i64> = stmt
        .query_map(params![shot_id], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().collect())
}

fn dismissed(db: &Database, shot_id: Option<i64>, prefix: &str) -> CoreResult<HashSet<String>> {
    let mut stmt = db.conn().prepare(
        "SELECT target FROM suggestion_feedback
         WHERE action = 'dismiss'
           AND (screenshot_id IS NULL OR screenshot_id = ?1)",
    )?;
    let sid = shot_id.unwrap_or(-1);
    let rows: Vec<String> = stmt
        .query_map(params![sid], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|t| t.starts_with(prefix))
        .collect())
}

/// Tier 0: score one unfiled screenshot against existing collections + tags.
pub fn suggest_for_screenshot(db: &Database, id: i64) -> CoreResult<ShotSuggestions> {
    let mut out = ShotSuggestions::default();
    let Some(mut shot) = shot_profile(db, id)? else {
        return Ok(out);
    };
    let burst_of = burst_membership(db)?;
    shot.burst = burst_of.get(&id).cloned();
    let mine = member_collections(db, id)?;
    let dismissed_cols = dismissed(db, Some(id), "collection:")?;
    let dismissed_tags = dismissed(db, Some(id), "tag:")?;

    let profiles = collection_profiles(db)?;
    let mut cols: Vec<ScoredTarget> = Vec::new();
    for p in &profiles {
        if mine.contains(&p.id) || p.member_ids.is_empty() {
            continue;
        }
        if dismissed_cols.contains(&format!("collection:{}", p.id)) {
            continue;
        }
        let mut score = 0i64;
        let mut reasons = Vec::new();
        if let (Some(a), Some(b)) = (shot.app.as_deref(), p.top_app.as_deref()) {
            if a == b {
                score += 3;
                reasons.push(format!("Same app: {a}"));
            }
        }
        if let (Some(a), Some(b)) = (shot.category.as_deref(), p.top_category.as_deref()) {
            if a == b {
                score += 2;
                reasons.push(format!("Same category: {a}"));
            }
        }
        if let (Some(a), Some(b)) = (shot.domain.as_deref(), p.top_domain.as_deref()) {
            if a == b {
                score += 2;
                reasons.push(format!("Same site: {a}"));
            }
        }
        let shared: Vec<&String> = shot.tags.intersection(&p.tags).collect();
        if !shared.is_empty() {
            let n = shared.len().min(3) as i64;
            score += n;
            reasons.push(format!(
                "{} shared tag{}",
                shared.len(),
                if shared.len() == 1 { "" } else { "s" }
            ));
        }
        if let Some(b) = shot.burst.as_deref() {
            if p.burst_keys.contains(b) {
                score += 2;
                reasons.push("Captured together".into());
            }
        }
        if let Some(ph) = shot.phash {
            if p.phashes.iter().any(|m| hamming_distance(*m, ph) <= PHASH_DISTANCE) {
                score += 2;
                reasons.push("Looks similar".into());
            }
        }
        if score >= MIN_COLLECTION_SCORE {
            cols.push(ScoredTarget {
                kind: "collection".into(),
                id: Some(p.id),
                name: p.name.clone(),
                score,
                reasons,
            });
        }
    }
    cols.sort_by(|a, b| b.score.cmp(&a.score).then(a.name.cmp(&b.name)));
    cols.truncate(MAX_TARGETS);

    // Tag profiles: top app/category/domain per tag, one grouped pass.
    let mut tstmt = db.conn().prepare(
        "SELECT t.name, s.app_name, s.category, s.website_domain
         FROM tags t
         JOIN screenshot_tags st ON st.tag_id = t.id
         JOIN screenshots s ON s.id = st.screenshot_id
         WHERE s.status = 'available'",
    )?;
    let tag_rows = tstmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut tag_apps: HashMap<String, Vec<Option<String>>> = HashMap::new();
    let mut tag_cats: HashMap<String, Vec<Option<String>>> = HashMap::new();
    let mut tag_doms: HashMap<String, Vec<Option<String>>> = HashMap::new();
    for (name, app, cat, dom) in &tag_rows {
        tag_apps.entry(name.clone()).or_default().push(app.clone());
        tag_cats.entry(name.clone()).or_default().push(cat.clone());
        tag_doms.entry(name.clone()).or_default().push(dom.clone());
    }
    let mut tags: Vec<ScoredTarget> = Vec::new();
    for name in tag_apps.keys() {
        if shot.tags.contains(name) || dismissed_tags.contains(&format!("tag:{name}")) {
            continue;
        }
        let mut score = 0i64;
        let mut reasons = Vec::new();
        let top_app = top_of(&tag_apps[name]);
        let top_cat = top_of(&tag_cats[name]);
        let top_dom = top_of(&tag_doms[name]);
        if let (Some(a), Some(b)) = (shot.app.as_deref(), top_app.as_deref()) {
            if a == b {
                score += 2;
                reasons.push(format!("Matches {name} usage in {a}"));
            }
        }
        if let (Some(a), Some(b)) = (shot.category.as_deref(), top_cat.as_deref()) {
            if a == b {
                score += 2;
                reasons.push(format!("Matches {name} {a} shots"));
            }
        }
        if let (Some(a), Some(b)) = (shot.domain.as_deref(), top_dom.as_deref()) {
            if a == b {
                score += 1;
                reasons.push(format!("Same site as {name}"));
            }
        }
        if score >= MIN_TAG_SCORE {
            tags.push(ScoredTarget {
                kind: "tag".into(),
                id: None,
                name: name.clone(),
                score,
                reasons,
            });
        }
    }
    tags.sort_by(|a, b| b.score.cmp(&a.score).then(a.name.cmp(&b.name)));
    tags.truncate(MAX_TARGETS);

    out.collections = cols;
    out.tags = tags;
    Ok(out)
}

/// Tier 1: cluster unfiled available shots (zero collections) by shared
/// (app, domain-or-category) signals into named collection proposals.
pub fn propose_groups(db: &Database) -> CoreResult<Vec<Proposal>> {
    let mut stmt = db.conn().prepare(
        "SELECT s.id, s.app_name, s.category, s.website_domain, s.content_hash,
                s.created_ts, s.modified_ts
         FROM screenshots s
         WHERE s.status = 'available'
           AND NOT EXISTS (SELECT 1 FROM collection_items ci WHERE ci.screenshot_id = s.id)
         ORDER BY s.id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // Group key prefers (app, domain), falls back to (app, category).
    // Shots with no naming signal are left out — never invent names.
    let mut groups: HashMap<String, Vec<(i64, Option<String>, Option<String>)>> = HashMap::new();
    for (id, app, cat, dom, hash, _c, _m) in &rows {
        let key = match (app.as_deref(), dom.as_deref(), cat.as_deref()) {
            (Some(a), Some(d), _) if !a.is_empty() && !d.is_empty() => {
                Some((format!("app:{a}\x1fdom:{d}"), a.to_string(), Some(d.to_string()), None))
            }
            (Some(a), _, Some(c)) if !a.is_empty() && !c.is_empty() => {
                Some((format!("app:{a}\x1fcat:{c}"), a.to_string(), None, Some(c.to_string())))
            }
            (None, _, Some(c)) if !c.is_empty() => {
                Some((format!("cat:{c}"), String::new(), None, Some(c.to_string())))
            }
            _ => None,
        };
        if let Some((k, _a, _d, _c)) = key {
            groups.entry(k).or_default().push((*id, hash.clone(), None));
        }
    }
    // Dismissed proposal keys stay dismissed.
    let dismissed_keys = dismissed(db, None, "proposal:")?;
    let mut proposals: Vec<Proposal> = Vec::new();
    let mut used: HashSet<i64> = HashSet::new();
    let mut keys: Vec<String> = groups.keys().cloned().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(groups[k].len()));
    for key in keys {
        if proposals.len() >= MAX_PROPOSALS {
            break;
        }
        if dismissed_keys.contains(&format!("proposal:{key}")) {
            continue;
        }
        let members: Vec<(i64, Option<String>)> = groups[&key]
            .iter()
            .filter(|(id, _, _)| !used.contains(id))
            .map(|(id, h, _)| (*id, h.clone()))
            .collect();
        if members.len() < MIN_PROPOSAL_SIZE {
            continue;
        }
        for (id, _) in &members {
            used.insert(*id);
        }
        let (app, dom, cat) = parse_group_key(&key);
        let title = if !app.is_empty() && dom.as_deref().map(|d| !d.is_empty()).unwrap_or(false) {
            format!("{} · {}", app, dom.clone().unwrap_or_default())
        } else if !app.is_empty() {
            format!("{} · {}", app, cat.clone().unwrap_or_default())
        } else {
            cat.clone().unwrap_or_default()
        };
        let mut reasons = Vec::new();
        if !app.is_empty() {
            reasons.push(format!("Same app: {app}"));
        }
        if let Some(d) = dom.as_deref() {
            if !d.is_empty() {
                reasons.push(format!("Same site: {d}"));
            }
        }
        if let Some(c) = cat.as_deref() {
            if !c.is_empty() && dom.is_none() {
                reasons.push(format!("Same category: {c}"));
            }
        }
        let member_ids: Vec<i64> = members
            .iter()
            .take(MAX_PROPOSAL_MEMBERS)
            .map(|(id, _)| *id)
            .collect();
        let preview_hashes: Vec<Option<String>> =
            members.iter().take(4).map(|(_, h)| h.clone()).collect();
        proposals.push(Proposal {
            key: key.clone(),
            name: format!("{} · {}", title, members.len()),
            reasons,
            member_count: members.len() as i64,
            member_ids,
            preview_hashes,
        });
    }
    Ok(proposals)
}

fn parse_group_key(key: &str) -> (String, Option<String>, Option<String>) {
    let mut app = String::new();
    let mut dom = None;
    let mut cat = None;
    for part in key.split('\x1f') {
        if let Some(v) = part.strip_prefix("app:") {
            app = v.to_string();
        } else if let Some(v) = part.strip_prefix("dom:") {
            dom = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("cat:") {
            cat = Some(v.to_string());
        }
    }
    (app, dom, cat)
}

/// Sidebar/banner overview: proposals plus Tier 0 coverage count.
pub fn suggestion_overview(db: &Database) -> CoreResult<SuggestOverview> {
    let proposals = propose_groups(db)?;
    // Unfiled available shots sharing an app with any non-empty collection
    // are strong Tier 0 candidates; count + smallest id for the banner.
    let row: (i64, Option<i64>) = db
        .conn()
        .query_row(
            "SELECT COUNT(*), MIN(s.id) FROM screenshots s
             WHERE s.status = 'available'
               AND NOT EXISTS (SELECT 1 FROM collection_items ci WHERE ci.screenshot_id = s.id)
               AND s.app_name IS NOT NULL
               AND EXISTS (
                 SELECT 1 FROM screenshots m
                 JOIN collection_items ci ON ci.screenshot_id = m.id
                 WHERE m.status = 'available' AND m.app_name = s.app_name
               )",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
    Ok(SuggestOverview {
        proposals,
        suggested_count: row.0,
        first_suggested_id: row.1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScreenshot;

    fn test_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        let enrich = |id: i64, app: &str, cat: &str, dom: &str| {
            db.conn()
                .execute(
                    "UPDATE screenshots SET app_name = ?1, category = ?2, website_domain = ?3 WHERE id = ?4",
                    rusqlite::params![app, cat, dom, id],
                )
                .unwrap();
        };
        // Collection "docker": two code/app shots tagged "ship".
        let col = db.create_collection("docker").unwrap();
        for (i, name) in ["d1.png", "d2.png"].iter().enumerate() {
            let id = db
                .insert_screenshot(&NewScreenshot {
                    path: format!("/tmp/{name}"),
                    filename: (*name).into(),
                    content_hash: Some(format!("h{i}")),
                    phash: Some(crate::hashing::phash_to_hex(0x1000 + i as u64)),
                    ..Default::default()
                })
                .unwrap();
            enrich(id, "code", "work", "github.com");
            db.add_tag(id, "ship").unwrap();
            if *name == "d1.png" {
                db.add_tag(id, "containers").unwrap();
            }
            db.add_to_collection(col.id, id).unwrap();
        }
        // Unfiled shot matching docker on app+category+domain+tag signals.
        let u1 = db
            .insert_screenshot(&NewScreenshot {
                path: "/tmp/u1.png".into(),
                filename: "u1.png".into(),
                content_hash: Some("hu1".into()),
                phash: Some(crate::hashing::phash_to_hex(0x1000)),
                ..Default::default()
            })
            .unwrap();
        enrich(u1, "code", "work", "github.com");
        db.add_tag(u1, "containers").unwrap();
        // Unrelated unfiled shot: different app, no overlap.
        let u2 = db
            .insert_screenshot(&NewScreenshot {
                path: "/tmp/u2.png".into(),
                filename: "u2.png".into(),
                ..Default::default()
            })
            .unwrap();
        enrich(u2, "spotify", "music", "");
        // Three more code/github shots -> Tier 1 proposal group.
        for name in ["p1.png", "p2.png", "p3.png"] {
            let id = db
                .insert_screenshot(&NewScreenshot {
                    path: format!("/tmp/{name}"),
                    filename: (*name).into(),
                    content_hash: Some(format!("hp{name}")),
                    ..Default::default()
                })
                .unwrap();
            enrich(id, "code", "work", "github.com");
        }
        db
    }

    #[test]
    fn tier0_scores_and_reasons() {
        let db = test_db();
        let u1: i64 = db
            .conn()
            .query_row("SELECT id FROM screenshots WHERE filename = 'u1.png'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let s = suggest_for_screenshot(&db, u1).unwrap();
        assert_eq!(s.collections.len(), 1);
        assert_eq!(s.collections[0].name, "docker");
        assert!(s.collections[0].score >= 4);
        assert!(s.collections[0].reasons.iter().any(|r| r.contains("code")));
        // Tag "ship" is suggested via app+category match.
        assert!(s.tags.iter().any(|t| t.name == "ship"));
        // Already-member shots get no self-suggestion.
        let d1: i64 = db
            .conn()
            .query_row("SELECT id FROM screenshots WHERE filename = 'd1.png'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(suggest_for_screenshot(&db, d1).unwrap().collections.is_empty());
        // Unrelated shot scores nothing.
        let u2: i64 = db
            .conn()
            .query_row("SELECT id FROM screenshots WHERE filename = 'u2.png'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let s2 = suggest_for_screenshot(&db, u2).unwrap();
        assert!(s2.collections.is_empty() && s2.tags.is_empty());
    }

    #[test]
    fn dismissals_and_thresholds_hold() {
        let db = test_db();
        let u1: i64 = db
            .conn()
            .query_row("SELECT id FROM screenshots WHERE filename = 'u1.png'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let col_id: i64 = db
            .conn()
            .query_row("SELECT id FROM collections WHERE name = 'docker'", [], |r| r.get(0))
            .unwrap();
        db.record_feedback(Some(u1), &format!("collection:{col_id}"), "dismiss")
            .unwrap();
        assert!(suggest_for_screenshot(&db, u1)
            .unwrap()
            .collections
            .is_empty());
        assert!(db.record_feedback(None, "", "dismiss").is_err());
        assert!(db.record_feedback(None, "x", "maybe").is_err());
    }

    #[test]
    fn tier1_proposes_named_groups() {
        let db = test_db();
        let proposals = propose_groups(&db).unwrap();
        assert!(!proposals.is_empty());
        let code = proposals.iter().find(|p| p.name.contains("code")).unwrap();
        assert!(code.member_count >= 3);
        assert!(code.preview_hashes.len() <= 4);
        assert!(!code.reasons.is_empty());
        // Filed shots never appear in proposals.
        let all: Vec<i64> = proposals.iter().flat_map(|p| p.member_ids.clone()).collect();
        let d1: i64 = db
            .conn()
            .query_row("SELECT id FROM screenshots WHERE filename = 'd1.png'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!all.contains(&d1));
        // Dismissing a proposal key hides it.
        db.record_feedback(None, &format!("proposal:{}", code.key), "dismiss")
            .unwrap();
        let again = propose_groups(&db).unwrap();
        assert!(again.iter().all(|p| p.key != code.key));
    }

    #[test]
    fn overview_counts_coverage() {
        let db = test_db();
        let o = suggestion_overview(&db).unwrap();
        // u1 + p1..p3 share app "code" with docker members.
        assert!(o.suggested_count >= 4);
        assert!(o.first_suggested_id.is_some());
    }

    #[test]
    fn origin_writes_and_auto_marking() {
        let db = test_db();
        let u1: i64 = db
            .conn()
            .query_row("SELECT id FROM screenshots WHERE filename = 'u1.png'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(db.add_tag_with_origin(u1, "via-suggest", "suggested").unwrap());
        let origin: String = db
            .conn()
            .query_row(
                "SELECT origin FROM screenshot_tags st
                 JOIN tags t ON t.id = st.tag_id
                 WHERE st.screenshot_id = ?1 AND t.name = 'via-suggest'",
                params![u1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(origin, "suggested");
        assert!(db.add_tag_with_origin(u1, "x", "bogus").is_err());
        let col = db.create_collection("auto-c").unwrap();
        assert!(db.mark_collection_auto(col.id, r#"{"kind":"suggested"}"#).unwrap());
        let kind: String = db
            .conn()
            .query_row("SELECT type FROM collections WHERE id = ?1", params![col.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "auto");
        assert_eq!(
            db.add_many_to_collection_with_origin(col.id, &[u1], "suggested")
                .unwrap(),
            1
        );
    }
}
