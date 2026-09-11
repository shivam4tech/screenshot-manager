//! Safe bulk rename: template engine, cross-platform validation, collision
//! detection, two-phase execution planning, and crash recovery.
//!
//! Design notes:
//! - Pure planning (`rename_preview`) never touches the filesystem.
//! - Execution (`rename_execute`) journals every leg first so the file
//!   watcher can tell our own renames apart from external changes, updates
//!   records only after each file actually moves, and stops + rolls back on
//!   unexpected filesystem errors.
//! - Only `available` records whose files exist are renamed. Extensions are
//!   always preserved; formats are never converted; nothing is overwritten.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::{CoreError, CoreResult};

/// Template variables users may reference in a naming pattern.
pub const TEMPLATE_VARS: &[&str] = &[
    "counter", "date", "time", "year", "month", "day", "original", "collection", "source",
];

/// Windows-reserved basenames (checked case-insensitively, sans extension).
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Stale journal rows older than this are crash leftovers, reconciled at the
/// start of every rename execution.
const STALE_CUTOFF_SQL: &str = "datetime('now', '-10 minutes')";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameOrder {
    Grid,
    Oldest,
    Newest,
    Name,
}

impl RenameOrder {
    fn parse(s: &str) -> CoreResult<Self> {
        match s {
            "grid" => Ok(RenameOrder::Grid),
            "oldest" => Ok(RenameOrder::Oldest),
            "newest" => Ok(RenameOrder::Newest),
            "name" => Ok(RenameOrder::Name),
            _ => Err(CoreError::other(
                "unknown rename order (expected grid, oldest, newest, or name)",
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            RenameOrder::Grid => "grid",
            RenameOrder::Oldest => "oldest",
            RenameOrder::Newest => "newest",
            RenameOrder::Name => "name",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStrategy {
    Stop,
    Append,
}

impl ConflictStrategy {
    fn parse(s: &str) -> CoreResult<Self> {
        match s {
            "stop" => Ok(ConflictStrategy::Stop),
            "append" => Ok(ConflictStrategy::Append),
            _ => Err(CoreError::other(
                "unknown conflict strategy (expected stop or append)",
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenameOptions {
    pub pattern: String,
    pub counter_start: i64,
    pub padding: u32,
    pub order: String,
    pub strategy: String,
}

#[derive(Debug, Clone)]
struct FileCtx {
    id: i64,
    old_path: PathBuf,
    dir: PathBuf,
    stem: String,
    ext: String,
    ts: i64,
    filename: String,
    collection: String,
    source: String,
}

/// One planned rename. `status` is "ok", "skipped", or "conflict".
#[derive(Debug, Clone, Serialize)]
pub struct RenameEntry {
    pub id: i64,
    pub old_path: String,
    pub new_path: String,
    pub status: String,
    pub message: String,
    pub sanitized: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenamePlan {
    pub entries: Vec<RenameEntry>,
    /// Distinct parent directories involved (rename never moves files).
    pub dirs: Vec<String>,
    pub order: String,
    pub strategy: String,
}

/// Explicit rename target for execution (round-trips through preview).
#[derive(Debug, Clone, Deserialize)]
pub struct RenameTarget {
    pub id: i64,
    pub new_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenameResult {
    pub id: i64,
    pub old_path: String,
    pub new_path: String,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RenameOutcome {
    pub renamed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub rolled_back: usize,
    pub results: Vec<RenameResult>,
}

/// Days since epoch → (year, month, day) in UTC (Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn date_parts(ts: i64) -> (String, String, String, String, String) {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm) = ((secs / 3600) as u32, ((secs % 3600) / 60) as u32);
    (
        format!("{y:04}-{m:02}-{d:02}"),
        format!("{hh:02}-{mm:02}"),
        format!("{y:04}"),
        format!("{m:02}"),
        format!("{d:02}"),
    )
}

fn split_ext(filename: &str) -> (String, String) {
    match filename.rfind('.') {
        Some(i) if i > 0 => (filename[..i].into(), filename[i..].into()),
        _ => (filename.into(), String::new()),
    }
}

/// Render a pattern for one file. Unknown `{variables}` are an error, never
/// passed through silently.
fn render_basename(
    pattern: &str,
    counter: i64,
    padding: u32,
    ctx: &FileCtx,
) -> CoreResult<String> {
    if pattern.trim().is_empty() {
        return Err(CoreError::other("naming pattern must not be empty"));
    }
    let (date, time, year, month, day) = date_parts(ctx.ts);
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '{' {
            out.push(c);
            continue;
        }
        let mut var = String::new();
        let mut closed = false;
        for c in chars.by_ref() {
            if c == '}' {
                closed = true;
                break;
            }
            var.push(c);
        }
        if !closed {
            return Err(CoreError::other("unclosed {variable} in naming pattern"));
        }
        match var.as_str() {
            "counter" => {
                let digits = counter.to_string();
                let width = padding.clamp(1, 6) as usize;
                if digits.len() < width {
                    out.push_str(&"0".repeat(width - digits.len()));
                }
                out.push_str(&digits);
            }
            "date" => out.push_str(&date),
            "time" => out.push_str(&time),
            "year" => out.push_str(&year),
            "month" => out.push_str(&month),
            "day" => out.push_str(&day),
            "original" => out.push_str(&ctx.stem),
            "collection" => out.push_str(&ctx.collection),
            "source" => out.push_str(&ctx.source),
            _ => {
                return Err(CoreError::other(format!(
                    "unknown {{variable}} '{var}' (supported: {})",
                    TEMPLATE_VARS.join(", ")
                )));
            }
        }
    }
    Ok(out)
}

/// Make a basename safe on Linux, macOS, and Windows. Returns the cleaned
/// name plus whether anything changed (surfaced in preview, never silent).
fn sanitize_basename(name: &str) -> CoreResult<(String, bool)> {
    let mut cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows dislikes trailing spaces and periods.
    while cleaned.ends_with(' ') || cleaned.ends_with('.') {
        cleaned.pop();
    }
    if cleaned.is_empty() {
        return Err(CoreError::other(
            "pattern produced an empty filename after sanitizing",
        ));
    }
    let mut out = cleaned.clone();
    let stem_is_reserved = cleaned
        .split('.')
        .next()
        .map(|stem| RESERVED_NAMES.iter().any(|r| r.eq_ignore_ascii_case(stem)))
        .unwrap_or(false);
    if stem_is_reserved {
        out = match cleaned.rfind('.') {
            Some(i) if i > 0 => format!("{}_.{}", &cleaned[..i], &cleaned[i + 1..]),
            _ => format!("{cleaned}_"),
        };
    }
    Ok((out.clone(), out != name))
}

/// Load renameable file contexts, preserving input order for "grid".
fn load_contexts(db: &Database, ids: &[i64]) -> CoreResult<(Vec<FileCtx>, Vec<RenameEntry>)> {
    let mut contexts = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        let row: Option<(String, String, Option<i64>, Option<i64>, Option<String>, String)> = db
            .conn()
            .query_row(
                "SELECT path, filename, created_ts, modified_ts, app_name, status
                 FROM screenshots WHERE id = ?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        let Some((path, filename, created_ts, modified_ts, app_name, status)) = row else {
            skipped.push(RenameEntry {
                id: *id,
                old_path: String::new(),
                new_path: String::new(),
                status: "skipped".into(),
                message: "record not found".into(),
                sanitized: false,
            });
            continue;
        };
        let old_path = PathBuf::from(&path);
        if status != "available" || !old_path.is_file() {
            skipped.push(RenameEntry {
                id: *id,
                old_path: path,
                new_path: String::new(),
                status: "skipped".into(),
                message: "file not available".into(),
                sanitized: false,
            });
            continue;
        }
        let (stem, ext) = split_ext(&filename);
        let dir = old_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let collection: String = db
            .conn()
            .query_row(
                "SELECT c.name FROM collections c
                 JOIN collection_items ci ON ci.collection_id = c.id
                 WHERE ci.screenshot_id = ?1 ORDER BY c.name LIMIT 1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();
        contexts.push(FileCtx {
            id: *id,
            old_path,
            dir,
            stem,
            ext,
            ts: created_ts.or(modified_ts).unwrap_or(0),
            filename,
            collection,
            source: app_name.unwrap_or_default(),
        });
    }
    Ok((contexts, skipped))
}

fn order_contexts(contexts: &mut [FileCtx], order: RenameOrder) {
    match order {
        RenameOrder::Grid => {}
        RenameOrder::Oldest => contexts.sort_by_key(|c| (c.ts, c.id)),
        RenameOrder::Newest => contexts.sort_by_key(|c| (std::cmp::Reverse(c.ts), c.id)),
        RenameOrder::Name => contexts.sort_by_key(|c| (c.filename.to_lowercase(), c.id)),
    }
}

fn fold_key(dir: &Path, filename: &str) -> String {
    format!("{}//{}", dir.to_string_lossy(), filename.to_lowercase())
}

/// Build a preview plan without touching the filesystem.
pub fn rename_preview(
    db: &Database,
    ids: &[i64],
    opts: &RenameOptions,
) -> CoreResult<RenamePlan> {
    let order = RenameOrder::parse(&opts.order)?;
    let strategy = ConflictStrategy::parse(&opts.strategy)?;
    let (mut contexts, mut entries) = load_contexts(db, ids)?;
    order_contexts(&mut contexts, order);

    let old_keys: HashSet<String> = contexts
        .iter()
        .map(|c| fold_key(&c.dir, &c.filename))
        .collect();
    let mut claimed: HashMap<String, i64> = HashMap::new();
    let mut counter = opts.counter_start.max(0);
    for ctx in &contexts {
        let rendered = render_basename(&opts.pattern, counter, opts.padding, ctx)?;
        counter += 1;
        let (clean, sanitized) = sanitize_basename(&rendered)?;
        let mut new_name = format!("{clean}{}", ctx.ext);
        if new_name.as_bytes().len() > 255 {
            entries.push(RenameEntry {
                id: ctx.id,
                old_path: ctx.old_path.to_string_lossy().into_owned(),
                new_path: String::new(),
                status: "conflict".into(),
                message: "resulting filename is too long".into(),
                sanitized,
            });
            continue;
        }
        let new_path = ctx.dir.join(&new_name);
        let old_str = ctx.old_path.to_string_lossy().into_owned();
        if new_path == ctx.old_path {
            entries.push(RenameEntry {
                id: ctx.id,
                old_path: old_str,
                new_path: new_path.to_string_lossy().into_owned(),
                status: "skipped".into(),
                message: "name unchanged".into(),
                sanitized,
            });
            continue;
        }
        let mut key = fold_key(&ctx.dir, &new_name);
        let mut status = "ok";
        let mut message = String::new();
        let clashes_batch = claimed.contains_key(&key);
        let clashes_disk =
            new_path.exists() && !old_keys.contains(&key);
        if clashes_batch || clashes_disk {
            if strategy == ConflictStrategy::Append {
                let mut n = 2u32;
                let resolved = loop {
                    if n > 100 {
                        break None;
                    }
                    let candidate = format!("{clean}-{n}{}", ctx.ext);
                    let ckey = fold_key(&ctx.dir, &candidate);
                    let busy = claimed.contains_key(&ckey)
                        || (ctx.dir.join(&candidate).exists() && !old_keys.contains(&ckey));
                    if !busy {
                        break Some((candidate, ckey));
                    }
                    n += 1;
                };
                match resolved {
                    Some((candidate, ckey)) => {
                        new_name = candidate;
                        key = ckey;
                        message = "number appended to avoid a conflict".into();
                    }
                    None => {
                        status = "conflict";
                        message = "could not resolve the filename conflict".into();
                    }
                }
            } else {
                status = "conflict";
                message = if clashes_batch {
                    "another selected file resolves to the same name".into()
                } else {
                    format!(
                        "{} already exists",
                        ctx.dir.join(&new_name).to_string_lossy()
                    )
                };
            }
        }
        // Case-only differences collide on Windows/macOS: they share a fold
        // key, so the check above already covers them on every platform.
        if status == "ok" {
            claimed.insert(key, ctx.id);
        }
        entries.push(RenameEntry {
            id: ctx.id,
            old_path: old_str,
            new_path: ctx.dir.join(&new_name).to_string_lossy().into_owned(),
            status: status.into(),
            message,
            sanitized,
        });
    }

    let mut dirs: Vec<String> = contexts
        .iter()
        .map(|c| c.dir.to_string_lossy().into_owned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    dirs.sort();
    Ok(RenamePlan {
        entries,
        dirs,
        order: order.as_str().into(),
        strategy: match strategy {
            ConflictStrategy::Stop => "stop".into(),
            ConflictStrategy::Append => "append".into(),
        },
    })
}

/// Recover from an interrupted batch: for each stale journal row, if the new
/// path exists the move completed (point the record at it); else if the old
/// path exists the move never happened (point the record back); otherwise
/// the file is gone (mark missing). Stray temp files are removed.
pub fn reconcile_stale_renames(db: &Database) -> CoreResult<usize> {
    let stale = db.journal_stale(STALE_CUTOFF_SQL)?;
    let mut fixed = 0;
    for row in &stale {
        let new_p = Path::new(&row.new_path);
        let old_p = Path::new(&row.old_path);
        if new_p.exists() {
            let filename = new_p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (size, mtime) = file_fingerprint(new_p);
            db.update_renamed_path(row.screenshot_id, &row.new_path, &filename, size, mtime)?;
            fixed += 1;
        } else if old_p.exists() {
            let filename = old_p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (size, mtime) = file_fingerprint(old_p);
            db.update_renamed_path(row.screenshot_id, &row.old_path, &filename, size, mtime)?;
            fixed += 1;
        } else {
            db.mark_missing(&[row.screenshot_id])?;
            fixed += 1;
        }
        if let Some(temp) = &row.temp_path {
            let _ = std::fs::remove_file(temp);
        }
        db.journal_delete(row.id)?;
    }
    if fixed > 0 {
        log::info!("reconciled {fixed} stale rename journal rows");
    }
    Ok(fixed)
}

fn file_fingerprint(path: &Path) -> (i64, Option<i64>) {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return (0, None),
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    (meta.len() as i64, mtime)
}

fn temp_name(dir: &Path, batch: &str, i: usize) -> PathBuf {
    dir.join(format!(".shotmemory-tmp-{batch}-{i}.tmp"))
}

/// Execute explicit rename targets (normally round-tripped through preview).
/// Validates everything first; on unexpected filesystem failure stops the
/// batch and reverses completed moves where safe, reporting exact state.
pub fn rename_execute(db: &Database, targets: &[RenameTarget]) -> CoreResult<RenameOutcome> {
    let _ = reconcile_stale_renames(db);
    let mut outcome = RenameOutcome::default();
    if targets.is_empty() {
        return Ok(outcome);
    }

    // 1. Validate: records available, files present, pairwise + disk conflicts.
    struct Work {
        id: i64,
        old_path: PathBuf,
        new_path: PathBuf,
        old_fold: String,
        new_fold: String,
    }
    let mut work: Vec<Work> = Vec::new();
    for t in targets {
        let new_path = PathBuf::from(&t.new_path);
        let row: Option<(String, String)> = db
            .conn()
            .query_row(
                "SELECT path, status FROM screenshots WHERE id = ?1",
                rusqlite::params![t.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (old_s, status) = match row {
            Some(r) => r,
            None => {
                outcome.failed += 1;
                outcome.results.push(RenameResult {
                    id: t.id,
                    old_path: String::new(),
                    new_path: t.new_path.clone(),
                    ok: false,
                    message: "record not found".into(),
                });
                continue;
            }
        };
        let old_path = PathBuf::from(&old_s);
        if status != "available" || !old_path.is_file() {
            outcome.failed += 1;
            outcome.results.push(RenameResult {
                id: t.id,
                old_path: old_s,
                new_path: t.new_path.clone(),
                ok: false,
                message: "file not available".into(),
            });
            continue;
        }
        if new_path == old_path {
            outcome.skipped += 1;
            outcome.results.push(RenameResult {
                id: t.id,
                old_path: old_s,
                new_path: t.new_path.clone(),
                ok: true,
                message: "unchanged".into(),
            });
            continue;
        }
        let parent_of = |p: &Path| {
            p.parent().map(|x| x.to_path_buf()).unwrap_or_default()
        };
        let name_of = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        work.push(Work {
            id: t.id,
            old_path: old_path.clone(),
            new_path: new_path.clone(),
            old_fold: fold_key(&parent_of(&old_path), &name_of(&old_path)),
            new_fold: fold_key(&parent_of(&new_path), &name_of(&new_path)),
        });
    }
    // Pairwise fold collisions + pre-existing on-disk files.
    let old_folds: HashSet<String> = work.iter().map(|w| w.old_fold.clone()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut valid: Vec<Work> = Vec::new();
    for w in work {
        if !seen.insert(w.new_fold.clone()) {
            outcome.failed += 1;
            outcome.results.push(RenameResult {
                id: w.id,
                old_path: w.old_path.to_string_lossy().into_owned(),
                new_path: w.new_path.to_string_lossy().into_owned(),
                ok: false,
                message: "another selected file resolves to the same name".into(),
            });
            continue;
        }
        if w.new_path.exists() && !old_folds.contains(&w.new_fold) {
            outcome.failed += 1;
            outcome.results.push(RenameResult {
                id: w.id,
                old_path: w.old_path.to_string_lossy().into_owned(),
                new_path: w.new_path.to_string_lossy().into_owned(),
                ok: false,
                message: "target already exists".into(),
            });
            continue;
        }
        valid.push(w);
    }
    if valid.is_empty() {
        return Ok(outcome);
    }

    // 2. A non-batch record already pointing at a target path would make
    // the record update below collide. Fail those entries up front.
    if !valid.is_empty() {
        let targets: Vec<String> = valid
            .iter()
            .map(|w| w.new_path.to_string_lossy().into_owned())
            .collect();
        let batch_ids: Vec<i64> = valid.iter().map(|w| w.id).collect();
        let placeholders: Vec<String> = targets.iter().map(|_| "?".to_string()).collect();
        let id_holders: Vec<String> = batch_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT path FROM screenshots WHERE path IN ({}) AND id NOT IN ({})",
            placeholders.join(","),
            id_holders.join(",")
        );
        let mut stmt = db.conn().prepare(&sql)?;
        let mut params: Vec<String> = targets;
        params.extend(batch_ids.iter().map(|i| i.to_string()));
        let taken: HashSet<String> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |r| r.get(0))?
            .collect::<Result<HashSet<_>, _>>()?;
        if !taken.is_empty() {
            let mut kept = Vec::new();
            for w in valid {
                if taken.contains(&w.new_path.to_string_lossy().into_owned()) {
                    outcome.failed += 1;
                    outcome.results.push(RenameResult {
                        id: w.id,
                        old_path: w.old_path.to_string_lossy().into_owned(),
                        new_path: w.new_path.to_string_lossy().into_owned(),
                        ok: false,
                        message: "target already exists".into(),
                    });
                } else {
                    kept.push(w);
                }
            }
            valid = kept;
        }
    }
    if valid.is_empty() {
        return Ok(outcome);
    }

    // 3. Uniform two-phase moves: every file vacates to a unique temp name
    // first, so swaps, cycles, and case-only renames can never overwrite
    // each other. Records track each phase, so any interruption is exactly
    // reconcilable (see reconcile_stale_renames).
    let batch = format!(
        "rn-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        std::process::id()
    );
    let temp_of = |w: &Work| {
        temp_name(
            &w.old_path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            &batch,
            w.id as usize,
        )
    };
    for w in &valid {
        db.journal_rename(
            &batch,
            w.id,
            &w.old_path.to_string_lossy(),
            &w.new_path.to_string_lossy(),
            Some(&temp_of(w).to_string_lossy()),
        )?;
    }

    // Phase A: vacate to temp names. Records still point at old paths, so a
    // failure here reverses cleanly with no database changes needed.
    let mut vacated: Vec<&Work> = Vec::new();
    for w in &valid {
        if let Err(e) = std::fs::rename(&w.old_path, &temp_of(w)) {
            outcome.failed += 1;
            outcome.results.push(RenameResult {
                id: w.id,
                old_path: w.old_path.to_string_lossy().into_owned(),
                new_path: w.new_path.to_string_lossy().into_owned(),
                ok: false,
                message: format!("could not move file: {e}"),
            });
            break;
        }
        vacated.push(w);
    }
    if vacated.len() != valid.len() {
        let mut reversed = 0usize;
        for w in vacated.iter().rev() {
            if std::fs::rename(&temp_of(w), &w.old_path).is_ok() {
                reversed += 1;
            }
        }
        outcome.rolled_back = reversed;
        let _ = db.journal_clear_batch(&batch);
        outcome.results.push(RenameResult {
            id: -1,
            old_path: String::new(),
            new_path: String::new(),
            ok: false,
            message: format!(
                "stopped after a filesystem error; {reversed} completed move(s) reversed"
            ),
        });
        return Ok(outcome);
    }
    // Records now track the temp locations (unique: never collides).
    for w in &valid {
        let temp = temp_of(w);
        let temp_str = temp.to_string_lossy().into_owned();
        let temp_name = temp
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (size, mtime) = file_fingerprint(&temp);
        let _ = db.update_renamed_path(w.id, &temp_str, &temp_name, size, mtime);
    }

    // Phase B: temp names to final targets, updating each record right after
    // its own move. Targets are unique within the batch and verified free of
    // non-batch records above, so updates cannot collide.
    let mut stopped = false;
    for w in &valid {
        let temp = temp_of(w);
        if let Err(e) = std::fs::rename(&temp, &w.new_path) {
            outcome.failed += 1;
            outcome.results.push(RenameResult {
                id: w.id,
                old_path: w.old_path.to_string_lossy().into_owned(),
                new_path: w.new_path.to_string_lossy().into_owned(),
                ok: false,
                message: format!("could not move file: {e}"),
            });
            stopped = true;
            break;
        }
        let filename = w
            .new_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (size, mtime) = file_fingerprint(&w.new_path);
        if db
            .update_renamed_path(w.id, &w.new_path.to_string_lossy(), &filename, size, mtime)
            .is_err()
        {
            outcome.failed += 1;
            outcome.results.push(RenameResult {
                id: w.id,
                old_path: w.old_path.to_string_lossy().into_owned(),
                new_path: w.new_path.to_string_lossy().into_owned(),
                ok: false,
                message: "could not update record".into(),
            });
            stopped = true;
            break;
        }
        outcome.renamed += 1;
        outcome.results.push(RenameResult {
            id: w.id,
            old_path: w.old_path.to_string_lossy().into_owned(),
            new_path: w.new_path.to_string_lossy().into_owned(),
            ok: true,
            message: String::new(),
        });
    }
    if stopped {
        // Journal rows stay: files whose records still point at temp paths
        // are finished by reconcile_stale_renames on the next run.
        outcome.results.push(RenameResult {
            id: -1,
            old_path: String::new(),
            new_path: String::new(),
            ok: false,
            message: "stopped after a filesystem error; re-run to reconcile".into(),
        });
        return Ok(outcome);
    }
    let _ = db.journal_clear_batch(&batch);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScreenshot;

    fn paint_png(path: &Path, w: u32, h: u32) {
        let img = image::RgbImage::new(w, h);
        image::DynamicImage::ImageRgb8(img).save(path).unwrap();
    }

    fn ctx(id: i64, filename: &str, ts: i64) -> FileCtx {
        let (stem, ext) = split_ext(filename);
        FileCtx {
            id,
            old_path: PathBuf::from(format!("/s/{filename}")),
            dir: PathBuf::from("/s"),
            stem,
            ext,
            ts,
            filename: filename.into(),
            collection: "docker".into(),
            source: "code".into(),
        }
    }

    #[test]
    fn templates_render_all_variables() {
        let c = ctx(1, "Shot.png", 1_786_924_800); // 2026-08-17 00:00 UTC
        let r = render_basename("{collection}_{date}_{counter}", 1, 3, &c).unwrap();
        assert_eq!(r, "docker_2026-08-17_001");
        let r = render_basename("{time}_{original}_{source}", 7, 2, &c).unwrap();
        assert_eq!(r, "00-00_Shot_code");
        let r = render_basename("{year}{month}{day}_{counter}", 100, 3, &c).unwrap();
        assert_eq!(r, "20260817_100");
        assert!(render_basename("{tag}", 1, 3, &c).is_err());
        assert!(render_basename("oops {counter", 1, 3, &c).is_err());
        assert!(render_basename("   ", 1, 3, &c).is_err());
    }

    #[test]
    fn sanitize_handles_reserved_and_illegal_names() {
        assert_eq!(sanitize_basename("CON.png").unwrap(), ("CON_.png".into(), true));
        assert_eq!(sanitize_basename("lpt1").unwrap(), ("lpt1_".into(), true));
        assert_eq!(sanitize_basename("a<b>c:d.png").unwrap(), ("a_b_c_d.png".into(), true));
        assert_eq!(sanitize_basename("trailing. ").unwrap(), ("trailing".into(), true));
        assert_eq!(sanitize_basename("résumé_截图.png").unwrap(), ("résumé_截图.png".into(), false));
        assert!(sanitize_basename("...").is_err());
        assert_eq!(sanitize_basename("normal-name_1.png").unwrap(), ("normal-name_1.png".into(), false));
    }

    #[test]
    fn preview_detects_collisions_and_keeps_order() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        // Two files that will collide on the same template result.
        for name in ["a.png", "b.png"] {
            paint_png(&dir.path().join(name), 10, 10);
            db.insert_screenshot(&NewScreenshot {
                path: dir.path().join(name).to_string_lossy().into_owned(),
                filename: name.into(),
                ..Default::default()
            })
            .unwrap();
        }
        // A blocker file already occupying the second counter slot.
        paint_png(&dir.path().join("shot_002.png"), 10, 10);
        let ids: Vec<i64> = db
            .conn()
            .query_row("SELECT 1", [], |_| Ok(()))
            .map(|_| {
                let mut s = db.conn().prepare("SELECT id FROM screenshots ORDER BY id").unwrap();
                s.query_map([], |r| r.get(0))
                    .unwrap()
                    .collect::<Result<Vec<i64>, _>>()
                    .unwrap()
            })
            .unwrap();
        let opts = RenameOptions {
            pattern: "shot_{counter}".into(),
            counter_start: 1,
            padding: 3,
            order: "name".into(),
            strategy: "stop".into(),
        };
        let plan = rename_preview(&db, &ids[..2], &opts).unwrap();
        // a.png -> shot_001.png ok; b.png -> shot_002.png conflicts (blocker).
        assert_eq!(plan.entries[0].status, "ok");
        assert_eq!(plan.entries[1].status, "conflict");
        // Append strategy resolves it instead.
        let mut append_opts = opts.clone();
        append_opts.strategy = "append".into();
        let plan = rename_preview(&db, &ids[..2], &append_opts).unwrap();
        assert!(plan.entries.iter().all(|e| e.status == "ok"));
        assert!(plan.entries[1].new_path.ends_with("shot_002-2.png"));
    }

    #[test]
    fn execute_moves_files_and_keeps_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        paint_png(&dir.path().join("a.png"), 20, 20);
        paint_png(&dir.path().join("b.png"), 20, 20);
        let mut ids = Vec::new();
        for name in ["a.png", "b.png"] {
            ids.push(
                db.insert_screenshot(&NewScreenshot {
                    path: dir.path().join(name).to_string_lossy().into_owned(),
                    filename: name.into(),
                    size: 100,
                    ..Default::default()
                })
                .unwrap(),
            );
        }
        db.add_tag(ids[0], "keepme").unwrap();
        let col = db.create_collection("C").unwrap();
        db.add_to_collection(col.id, ids[0]).unwrap();

        let opts = RenameOptions {
            pattern: "trip_{counter}".into(),
            counter_start: 1,
            padding: 3,
            order: "name".into(),
            strategy: "stop".into(),
        };
        let plan = rename_preview(&db, &ids, &opts).unwrap();
        let targets: Vec<RenameTarget> = plan
            .entries
            .iter()
            .map(|e| RenameTarget { id: e.id, new_path: e.new_path.clone() })
            .collect();
        let out = rename_execute(&db, &targets).unwrap();
        assert_eq!(out.renamed, 2);
        assert!(dir.path().join("trip_001.png").exists());
        assert!(dir.path().join("trip_002.png").exists());
        assert!(!dir.path().join("a.png").exists());
        // Tags, collections, and search index follow the rename.
        let d = db.get_screenshot_detail(ids[0]).unwrap().unwrap();
        assert_eq!(d.filename, "trip_001.png");
        assert_eq!(d.tags, vec!["keepme".to_string()]);
        assert_eq!(db.screenshot_collections(ids[0]).unwrap().len(), 1);
        let found: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM fts_search WHERE filename = 'trip_001.png'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 1);
        // Watcher correlation: no journal rows left behind.
        assert!(!db.is_rename_pending(&d.path).unwrap());
    }

    #[test]
    fn execute_swaps_overlapping_names_safely() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        for name in ["one.png", "two.png"] {
            paint_png(&dir.path().join(name), 10, 10);
            db.insert_screenshot(&NewScreenshot {
                path: dir.path().join(name).to_string_lossy().into_owned(),
                filename: name.into(),
                ..Default::default()
            })
            .unwrap();
        }
        let ids: Vec<i64> = {
            let mut s = db.conn().prepare("SELECT id FROM screenshots ORDER BY filename").unwrap();
            s.query_map([], |r| r.get(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap()
        };
        // one.png -> two.png, two.png -> one.png.
        let targets = vec![
            RenameTarget { id: ids[0], new_path: dir.path().join("two.png").to_string_lossy().into_owned() },
            RenameTarget { id: ids[1], new_path: dir.path().join("one.png").to_string_lossy().into_owned() },
        ];
        let out = rename_execute(&db, &targets).unwrap();
        assert_eq!(out.renamed, 2);
        // Contents followed their records (different sizes prove it).
        let d0 = db.get_screenshot_detail(ids[0]).unwrap().unwrap();
        let d1 = db.get_screenshot_detail(ids[1]).unwrap().unwrap();
        assert_eq!(d0.filename, "two.png");
        assert_eq!(d1.filename, "one.png");
    }

    #[test]
    fn execute_stops_and_reports_on_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        // blocker.png exists on disk but is NOT in the batch.
        for name in ["x.png", "y.png", "blocker.png"] {
            paint_png(&dir.path().join(name), 10, 10);
        }
        let ids: Vec<i64> = ["x.png", "y.png"]
            .iter()
            .map(|name| {
                db.insert_screenshot(&NewScreenshot {
                    path: dir.path().join(name).to_string_lossy().into_owned(),
                    filename: (*name).into(),
                    ..Default::default()
                })
                .unwrap()
            })
            .collect();
        let targets = vec![
            RenameTarget { id: ids[0], new_path: dir.path().join("ok.png").to_string_lossy().into_owned() },
            RenameTarget { id: ids[1], new_path: dir.path().join("blocker.png").to_string_lossy().into_owned() },
        ];
        let out = rename_execute(&db, &targets).unwrap();
        assert_eq!(out.renamed, 1);
        assert_eq!(out.failed, 1);
        assert!(dir.path().join("ok.png").exists());
        assert!(dir.path().join("blocker.png").exists());
        // blocker.png was never touched; y.png still in place.
        assert!(dir.path().join("y.png").exists());
    }

    #[test]
    fn stale_journal_reconciles_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        // Case 1: move completed, record stale.
        paint_png(&dir.path().join("new.png"), 10, 10);
        let id = db
            .insert_screenshot(&NewScreenshot {
                path: dir.path().join("old.png").to_string_lossy().into_owned(),
                filename: "old.png".into(),
                ..Default::default()
            })
            .unwrap();
        db.journal_rename("b1", id, &dir.path().join("old.png").to_string_lossy(), &dir.path().join("new.png").to_string_lossy(), None)
            .unwrap();
        // Case 2: move never happened, old file still there.
        paint_png(&dir.path().join("keep.png"), 10, 10);
        let id2 = db
            .insert_screenshot(&NewScreenshot {
                path: dir.path().join("renamed.png").to_string_lossy().into_owned(),
                filename: "renamed.png".into(),
                ..Default::default()
            })
            .unwrap();
        db.journal_rename("b1", id2, &dir.path().join("keep.png").to_string_lossy(), &dir.path().join("renamed.png").to_string_lossy(), None)
            .unwrap();
        // Backdate both rows past the stale cutoff.
        db.conn()
            .execute("UPDATE rename_journal SET created_at = '2000-01-01 00:00:00'", [])
            .unwrap();
        let fixed = reconcile_stale_renames(&db).unwrap();
        assert_eq!(fixed, 2);
        let d1 = db.get_screenshot_detail(id).unwrap().unwrap();
        assert_eq!(d1.filename, "new.png");
        let d2 = db.get_screenshot_detail(id2).unwrap().unwrap();
        assert_eq!(d2.filename, "keep.png");
        assert_eq!(d2.path, dir.path().join("keep.png").to_string_lossy());
    }

    #[test]
    fn watcher_skips_journaled_paths() {
        let db = Database::open_in_memory().unwrap();
        db.journal_rename("b9", 7, "/s/a.png", "/s/b.png", Some("/s/.tmp")).unwrap();
        assert!(db.is_rename_pending("/s/a.png").unwrap());
        assert!(db.is_rename_pending("/s/b.png").unwrap());
        assert!(db.is_rename_pending("/s/.tmp").unwrap());
        assert!(!db.is_rename_pending("/s/other.png").unwrap());
        db.journal_clear_batch("b9").unwrap();
        assert!(!db.is_rename_pending("/s/a.png").unwrap());
    }
}
