//! Search: lightweight query parsing + deterministic ranked search over FTS5.
//!
//! Query syntax (all optional, combinable):
//!
//! ```text
//! docker error                free words (FTS match across filename/OCR/tags/…)
//! "morphogenetic computation" exact phrase
//! after:2026-07-01            captured on/after a date (ISO yyyy-mm-dd)
//! before:2026-08-01           captured before a date
//! app:chrome                  source application (where detectable)
//! site:github.com             website/domain
//! tag:research                manual/auto tag
//! collection:business         collection membership
//! dir:/home/me/Screenshots    under a directory
//! type:png                    file format
//! has:text / has:notext      OCR text present / absent
//! is:duplicate                part of an exact-duplicate group
//! in august, from july        human month names → date ranges
//! last week, this month, yesterday, today … relative ranges
//! ```
//!
//! Ranking is deterministic and explainable: weighted bm25 (tags > filename >
//! note > OCR text > app/site) + exact-phrase bonus + modest recency boost.
//! No LLM involved.

use rusqlite::params_from_iter;
use serde::Serialize;

use crate::db::{Database, ScreenshotRow};
use crate::error::CoreResult;

/// A structured filter extracted from the query string.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum Filter {
    /// Unix seconds — captured on/after.
    After(i64),
    /// Unix seconds — captured before.
    Before(i64),
    App(String),
    Site(String),
    Tag(String),
    Collection(String),
    /// Directory prefix (normalized path).
    Dir(String),
    /// File format, e.g. "png".
    Format(String),
    /// OCR text present (true) / absent (false).
    HasText(bool),
    /// Member of an exact-duplicate group.
    Duplicates(bool),
}

/// The result of parsing a raw query string.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedQuery {
    /// Raw input, for display.
    pub raw: String,
    /// FTS5 MATCH expression built from free terms and quoted phrases.
    pub match_expr: Option<String>,
    /// Quoted phrases (used for the exact-phrase ranking bonus).
    pub phrases: Vec<String>,
    pub filters: Vec<Filter>,
}

impl ParsedQuery {
    pub fn is_empty(&self) -> bool {
        self.match_expr.is_none() && self.filters.is_empty()
    }
}

/// A parsed token from the query string.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Phrase(String),
    KeyVal(String, String),
}

/// Query parser with an injectable "now" clock for deterministic tests.
pub struct QueryParser {
    /// Unix seconds used to resolve relative dates.
    pub now: i64,
}

impl QueryParser {
    pub fn new(now: i64) -> Self {
        Self { now }
    }

    /// Parse a raw query string into structured parts.
    pub fn parse(&self, input: &str) -> ParsedQuery {
        let tokens = self.tokenize(input);
        let mut terms: Vec<String> = Vec::new();
        let mut phrases: Vec<String> = Vec::new();
        let mut filters: Vec<Filter> = Vec::new();

        let mut i = 0;
        while i < tokens.len() {
            match &tokens[i] {
                Token::Phrase(p) => {
                    let p = p.trim().to_string();
                    if !p.is_empty() {
                        phrases.push(p);
                    }
                }
                Token::KeyVal(key, val) => {
                    let val = val.trim().to_string();
                    if !val.is_empty() {
                        match self.keyval_to_filter(key, &val) {
                            Some(f) => filters.push(f),
                            // Unknown keys degrade to plain words so nothing
                            // the user typed silently vanishes.
                            None => terms.push(sanitize_term(&format!("{key} {val}"))),
                        }
                    }
                }
                Token::Word(w) => {
                    if let Some((after, before)) = self.try_date_word_pair(&tokens, i, &mut i) {
                        if let Some(a) = after {
                            filters.push(Filter::After(a));
                        }
                        if let Some(b) = before {
                            filters.push(Filter::Before(b));
                        }
                        continue;
                    }
                    if let Some((after, before)) = self.resolve_standalone(&w.to_ascii_lowercase())
                    {
                        if let Some(a) = after {
                            filters.push(Filter::After(a));
                        }
                        if let Some(b) = before {
                            filters.push(Filter::Before(b));
                        }
                        i += 1;
                        continue;
                    }
                    terms.push(sanitize_term(w));
                }
            }
            i += 1;
        }

        terms.retain(|t| !t.is_empty());
        let match_expr = build_match_expr(&terms, &phrases);
        ParsedQuery {
            raw: input.to_string(),
            match_expr,
            phrases,
            filters,
        }
    }

    fn keyval_to_filter(&self, key: &str, val: &str) -> Option<Filter> {
        match key.to_ascii_lowercase().as_str() {
            "after" | "since" => {
                parse_iso_date(val).map(|d| Filter::After(d * 86_400))
            }
            "before" | "until" => {
                parse_iso_date(val).map(|d| Filter::Before(d * 86_400))
            }
            "app" | "application" => Some(Filter::App(val.to_string())),
            "site" | "domain" | "url" => Some(Filter::Site(val.to_string())),
            "tag" => Some(Filter::Tag(val.to_string())),
            "collection" => Some(Filter::Collection(val.to_string())),
            "dir" | "folder" | "path" => {
                let normalized = crate::db::normalize_path(std::path::Path::new(val));
                Some(Filter::Dir(normalized))
            }
            "type" | "format" => Some(Filter::Format(val.to_string())),
            "has" => match val.to_ascii_lowercase().as_str() {
                "text" | "ocr" => Some(Filter::HasText(true)),
                "notext" | "no-text" => Some(Filter::HasText(false)),
                _ => None,
            },
            "is" => match val.to_ascii_lowercase().as_str() {
                "duplicate" | "dup" => Some(Filter::Duplicates(true)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Recognize date phrases that span two word tokens:
    /// "in august", "from july", "last week", "this month", …
    /// On success advances `i` past the consumed tokens and returns the range.
    fn try_date_word_pair(
        &self,
        tokens: &[Token],
        i: usize,
        consumed_to: &mut usize,
    ) -> Option<(Option<i64>, Option<i64>)> {
        let first = match &tokens[i] {
            Token::Word(w) => w.to_ascii_lowercase(),
            _ => return None,
        };
        let second = match tokens.get(i + 1) {
            Some(Token::Word(w)) => w.to_ascii_lowercase(),
            _ => return None,
        };

        let range = match first.as_str() {
            "in" | "from" => self.resolve_time_phrase(&second),
            "last" => self.resolve_relative(&second, 1),
            "this" => self.resolve_relative(&second, 0),
            _ => None,
        };
        if range.is_some() {
            *consumed_to = i + 2;
        }
        range
    }

    /// Resolve a bare word like "august", "december", "week" (for
    /// "in august" / "this week" contexts).
    fn resolve_time_phrase(&self, word: &str) -> Option<(Option<i64>, Option<i64>)> {
        if let Some(month) = month_number(word) {
            return Some(month_range(month, self.now));
        }
        None
    }

    /// Resolve "this X" (ago = 0) and "last X" (ago = 1) where X is
    /// week / month / year / weekend (treated as this week).
    fn resolve_relative(
        &self,
        word: &str,
        ago: u32,
    ) -> Option<(Option<i64>, Option<i64>)> {
        const DAY: i64 = 86_400;
        let days = self.now.div_euclid(DAY); // civil days since epoch
        match word {
            "week" | "wk" => {
                let after = (days - 7 * (ago as i64 + 1) + 1) * DAY;
                let before = (days - 7 * ago as i64 + 1) * DAY;
                Some((Some(after), Some(before)))
            }
            "month" => {
                let (y, m, _) = civil_from_days(days - 30 * ago as i64);
                let after = days_from_civil(y, m, 1) * DAY;
                let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                let before = days_from_civil(ny, nm, 1) * DAY;
                Some((Some(after), Some(before)))
            }
            "year" => {
                let y = civil_from_days(days - 365 * ago as i64).0;
                let after = days_from_civil(y, 1, 1) * DAY;
                let before = days_from_civil(y + 1, 1, 1) * DAY;
                Some((Some(after), Some(before)))
            }
            _ => None,
        }
    }

    /// Standalone time words: "today", "yesterday".
    fn resolve_standalone(&self, word: &str) -> Option<(Option<i64>, Option<i64>)> {
        const DAY: i64 = 86_400;
        let today_start = self.now.div_euclid(DAY) * DAY;
        match word {
            "today" => Some((Some(today_start), None)),
            "yesterday" => Some((Some(today_start - DAY), Some(today_start))),
            _ => None,
        }
    }

    /// Tokenize into words, quoted phrases and key:value pairs.
    fn tokenize(&self, input: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut chars = input.chars().peekable();
        while let Some(&c) = chars.peek() {
            match c {
                c if c.is_whitespace() => {
                    chars.next();
                }
                '"' | '\'' => {
                    let quote = c;
                    chars.next();
                    let mut phrase = String::new();
                    for ch in chars.by_ref() {
                        if ch == quote {
                            break;
                        }
                        phrase.push(ch);
                    }
                    tokens.push(Token::Phrase(phrase));
                }
                _ => {
                    let mut word = String::new();
                    while let Some(&ch) = chars.peek() {
                        if ch.is_whitespace() {
                            break;
                        }
                        // Allow quoted values inside key:value pairs.
                        if ch == '"' || ch == '\'' {
                            let quote = ch;
                            chars.next();
                            for inner in chars.by_ref() {
                                if inner == quote {
                                    break;
                                }
                                word.push(inner);
                            }
                        } else {
                            word.push(ch);
                            chars.next();
                        }
                    }
                    if let Some((key, val)) = word.split_once(':') {
                        tokens.push(Token::KeyVal(key.to_string(), val.to_string()));
                    } else {
                        tokens.push(Token::Word(word));
                    }
                }
            }
        }
        tokens
    }
}

/// Keep alphanumeric (unicode-aware) characters; drop FTS5 operator syntax
/// so user input can never inject query grammar. OCR text is untrusted input.
fn sanitize_term(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Build the FTS5 MATCH expression: terms AND-ed, phrases quoted.
fn build_match_expr(terms: &[String], phrases: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for t in terms {
        if !t.is_empty() {
            parts.push(format!("\"{t}\""));
        }
    }
    for p in phrases {
        let clean = sanitize_term(p);
        if !clean.is_empty() {
            parts.push(format!("\"{clean}\""));
        }
    }
    if parts.is_empty() {
        None
    } else {
        // Implicit AND between parts: all words/phrases must appear.
        Some(parts.join(" "))
    }
}

/// Parse `yyyy-mm-dd` into days-since-epoch.
fn parse_iso_date(s: &str) -> Option<i64> {
    let mut it = s.trim().split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// Month name/abbreviation → 1..=12.
fn month_number(word: &str) -> Option<u32> {
    let w = word.trim_end_matches('.').to_ascii_lowercase();
    let m = match w.as_str() {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "sept" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => return None,
    };
    Some(m)
}

/// Date range for a month name relative to `now`. If that month would be in
/// the future this year, assume the user means last year.
fn month_range(month: u32, now: i64) -> (Option<i64>, Option<i64>) {
    const DAY: i64 = 86_400;
    let (cy, cm, _) = civil_from_days(now.div_euclid(DAY));
    let year = if (month as i64) > cm { cy - 1 } else { cy };
    let after = days_from_civil(year, month as i64, 1) * DAY;
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month as i64 + 1) };
    let before = days_from_civil(ny, nm, 1) * DAY;
    (Some(after), Some(before))
}

/// Days-from-civil (Howard Hinnant's algorithm). No chrono dependency.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// One search result row: screenshot fields + OCR snippet + relevance score.
#[derive(Debug, Clone, Serialize)]
pub struct SearchRow {
    #[serde(flatten)]
    pub row: ScreenshotRow,
    /// Highlighted OCR snippet (match marks in `[` `]`).
    pub snippet: Option<String>,
    /// Deterministic relevance score (higher = better). Exposed so results
    /// stay explainable; not an LLM opinion.
    pub score: f64,
}

/// Complete outcome of a search execution.
#[derive(Debug, Clone, Serialize)]
pub struct SearchOutcome {
    pub total: i64,
    pub rows: Vec<SearchRow>,
    pub parsed: ParsedQuery,
}

/// bm25 column weights: (unindexed id, filename, ocr_text, tags, note, app, site).
/// Tags and filename matter more than body text; app/site carry mild weight.
const BM25_WEIGHTS: &str = "bm25(fts_search, 0.0, 8.0, 1.0, 12.0, 4.0, 6.0, 6.0)";

const BASE_COLS: &str = "s.id, s.path, s.filename, s.created_ts, s.width, s.height, s.format, \
s.status, s.ocr_status, s.content_hash, s.phash, s.starred";

/// Executes parsed queries against the database.
pub struct Searcher<'a> {
    db: &'a Database,
}

impl<'a> Searcher<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Parse and run `query`, returning ranked results.
    pub fn search(&self, query: &str, limit: i64, offset: i64) -> CoreResult<SearchOutcome> {
        let parser = QueryParser::new(now_secs());
        let parsed = parser.parse(query);
        self.execute(&parsed, limit, offset)
    }

    /// Run an already-parsed query.
    pub fn execute(
        &self,
        parsed: &ParsedQuery,
        limit: i64,
        offset: i64,
    ) -> CoreResult<SearchOutcome> {
        let mut where_clauses: Vec<String> = vec!["s.status != 'missing'".into()];
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        let mut extra_select = String::new();
        let mut order_score;
        let mut join_fts = String::new();

        if let Some(match_expr) = &parsed.match_expr {
            args.push(match_expr.clone().into());
            let match_param = args.len();
            join_fts = format!(
                "JOIN fts_search f ON f.screenshot_id = s.id AND fts_search MATCH ?{match_param}"
            );
            extra_select = format!(
                ", (- {BM25_WEIGHTS}) AS relevance, snippet(fts_search, 2, '[', ']', '…', 16) AS snip"
            );
            // Exact-phrase bonus (first phrase) + modest recency boost.
            let phrase_boost = if let Some(first) = parsed.phrases.first() {
                args.push(sanitize_term(first).into());
                format!(
                    "(CASE WHEN instr(lower(f.ocr_text), ?{0}) > 0 THEN 100 ELSE 0 END)",
                    args.len()
                )
            } else {
                "0".to_string()
            };
            args.push((now_secs() - 7 * 86_400).into());
            args.push((now_secs() - 30 * 86_400).into());
            order_score = format!(
                " ORDER BY (CASE WHEN s.created_ts >= ?{r1} THEN 5 ELSE 0 END \
                 + CASE WHEN s.created_ts >= ?{r2} THEN 3 ELSE 0 END \
                 + {phrase_boost} + relevance) DESC, s.created_ts DESC",
                r1 = args.len() - 1,
                r2 = args.len(),
            );
        } else {
            extra_select = ", 0.0 AS relevance, NULL AS snip".into();
            order_score =
                " ORDER BY COALESCE(s.created_ts, s.modified_ts) DESC, s.id DESC".into();
        }

        for f in &parsed.filters {
            match f {
                Filter::After(ts) => {
                    args.push((*ts).into());
                    where_clauses.push(format!(
                        "COALESCE(s.created_ts, s.modified_ts) >= ?{}",
                        args.len()
                    ));
                }
                Filter::Before(ts) => {
                    args.push((*ts).into());
                    where_clauses.push(format!(
                        "COALESCE(s.created_ts, s.modified_ts) < ?{}",
                        args.len()
                    ));
                }
                Filter::App(v) => {
                    args.push(format!("%{v}%").into());
                    where_clauses.push(format!("s.app_name LIKE ?{}", args.len()));
                }
                Filter::Site(v) => {
                    let n = args.len() + 1;
                    args.push(format!("%{v}%").into());
                    where_clauses.push(format!(
                        "(s.website_domain LIKE ?{n} OR s.url LIKE ?{n})"
                    ));
                }
                Filter::Tag(v) => {
                    args.push(format!("%{v}%").into());
                    where_clauses.push(format!(
                        "EXISTS (SELECT 1 FROM screenshot_tags st JOIN tags t ON t.id = st.tag_id \
                         WHERE st.screenshot_id = s.id AND t.name LIKE ?{})",
                        args.len()
                    ));
                }
                Filter::Collection(v) => {
                    args.push(format!("%{v}%").into());
                    where_clauses.push(format!(
                        "EXISTS (SELECT 1 FROM collection_items ci JOIN collections c \
                         ON c.id = ci.collection_id \
                         WHERE ci.screenshot_id = s.id AND c.name LIKE ?{})",
                        args.len()
                    ));
                }
                Filter::Dir(v) => {
                    args.push(format!("{v}/%").into());
                    where_clauses.push(format!("s.path LIKE ?{}", args.len()));
                }
                Filter::Format(v) => {
                    args.push(v.to_ascii_lowercase().into());
                    where_clauses.push(format!("lower(s.format) = ?{}", args.len()));
                }
                Filter::HasText(want) => {
                    where_clauses.push(format!(
                        "EXISTS (SELECT 1 FROM ocr_text o WHERE o.screenshot_id = s.id \
                         AND length(trim(o.text)) > 0) = {}",
                        if *want { 1 } else { 0 }
                    ));
                }
                Filter::Duplicates(true) => where_clauses.push(
                    "s.content_hash IN (SELECT content_hash FROM screenshots \
                     WHERE content_hash IS NOT NULL GROUP BY content_hash HAVING COUNT(*) > 1)"
                        .into(),
                ),
                Filter::Duplicates(false) => {}
            }
        }

        let where_sql = where_clauses.join(" AND ");
        args.push(limit.into());
        args.push(offset.into());
        let lim = args.len() - 1;
        let off = args.len();

        let where_sql = where_clauses.join(" AND ");
        args.push(limit.into());
        args.push(offset.into());
        let lim = args.len() - 1;
        let off = args.len();

        // Total count with the same filters (for "N results").
        let count_sql =
            format!("SELECT COUNT(*) FROM screenshots s {join_fts} WHERE {where_sql}");
        let total: i64 = self
            .db
            .conn()
            .query_row(&count_sql, params_from_iter(args.iter()), |r| r.get(0))?;

        // Page query.
        let page_sql = format!(
            "SELECT {BASE_COLS}{extra_select} \
             FROM screenshots s {join_fts} \
             WHERE {where_sql} \
             {order_score} \
             LIMIT ?{lim} OFFSET ?{off}"
        );
        let mut stmt = self.db.conn().prepare(&page_sql)?;
        let rows = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                Ok(SearchRow {
                    row: ScreenshotRow {
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
                    },
                    snippet: r.get::<_, Option<String>>(13)?,
                    score: r.get::<_, Option<f64>>(12)?.unwrap_or(0.0),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(SearchOutcome {
            total,
            rows,
            parsed: parsed.clone(),
        })
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScreenshot;

    // Fixed clock: 2026-09-02 12:00:00 UTC.
    const NOW: i64 = 1_787_713_600;

    fn insert(db: &Database, path: &str, filename: &str, created_days_ago: i64) -> i64 {
        db.insert_screenshot(&NewScreenshot {
            path: path.into(),
            filename: filename.into(),
            created_ts: Some(NOW - created_days_ago * 86_400),
            modified_ts: Some(NOW - created_days_ago * 86_400),
            format: Some("png".into()),
            ..Default::default()
        })
        .unwrap()
    }

    fn set_ocr(db: &Database, id: i64, text: &str) {
        db.fts_set_ocr(id, text).unwrap();
        db.conn()
            .execute(
                "INSERT INTO ocr_text(screenshot_id, text, confidence) VALUES (?1, ?2, 0.9)",
                rusqlite::params![id, text],
            )
            .unwrap();
        db.conn()
            .execute(
                "UPDATE screenshots SET ocr_status = 'done' WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
    }

    fn add_tag(db: &Database, id: i64, tag: &str) {
        db.conn()
            .execute("INSERT OR IGNORE INTO tags(name) VALUES (?1)", rusqlite::params![tag])
            .unwrap();
        db.conn()
            .execute(
                "INSERT OR IGNORE INTO screenshot_tags(screenshot_id, tag_id, origin)
                 SELECT ?1, id, 'manual' FROM tags WHERE name = ?2",
                rusqlite::params![id, tag],
            )
            .unwrap();
    }

    #[test]
    fn parser_plain_terms_and_phrases() {
        let p = QueryParser::new(NOW);
        let q = p.parse("docker error");
        assert_eq!(q.match_expr.as_deref(), Some("\"docker\" \"error\""));
        assert!(q.filters.is_empty());

        let q = p.parse("\"morphogenetic computation\" python");
        assert_eq!(q.phrases, vec!["morphogenetic computation"]);
        assert_eq!(q.match_expr.as_deref(), Some("\"python\" \"morphogeneticcomputation\""));

        // FTS operators are defused: user input can't inject query grammar.
        let q = p.parse("docker\" OR 1=1 --");
        assert!(!q.match_expr.as_deref().unwrap_or("").contains("OR 1=1"));
    }

    #[test]
    fn parser_keyval_filters() {
        let p = QueryParser::new(NOW);
        let q = p.parse("pricing after:2026-07-01 before:2026-08-01 app:chrome site:github.com tag:research type:png has:text is:duplicate");

        assert!(q.filters.contains(&Filter::After(days_from_civil(2026, 7, 1) * 86_400)));
        assert!(q.filters.contains(&Filter::Before(days_from_civil(2026, 8, 1) * 86_400)));
        assert!(q.filters.contains(&Filter::App("chrome".into())));
        assert!(q.filters.contains(&Filter::Site("github.com".into())));
        assert!(q.filters.contains(&Filter::Tag("research".into())));
        assert!(q.filters.contains(&Filter::Format("png".into())));
        assert!(q.filters.contains(&Filter::HasText(true)));
        assert!(q.filters.contains(&Filter::Duplicates(true)));
        assert_eq!(q.match_expr.as_deref(), Some("\"pricing\""));
    }

    #[test]
    fn parser_human_dates() {
        let p = QueryParser::new(NOW); // Sep 2, 2026
        let q = p.parse("github in august");
        // August 2026: Aug 1 .. Sep 1
        assert!(q.filters.contains(&Filter::After(days_from_civil(2026, 8, 1) * 86_400)));
        assert!(q.filters.contains(&Filter::Before(days_from_civil(2026, 9, 1) * 86_400)));
        assert_eq!(q.match_expr.as_deref(), Some("\"github\""));
        // "in august" words must not leak into terms
        assert!(!q.match_expr.unwrap().contains("august"));

        // A month later in the year wraps to last year
        let p2 = QueryParser::new(NOW);
        let q2 = p2.parse("in january");
        assert!(q2.filters.contains(&Filter::After(days_from_civil(2026, 1, 1) * 86_400)));

        let q3 = p.parse("last week");
        assert!(matches!(q3.filters.first(), Some(Filter::After(_))));

        let q4 = p.parse("error yesterday");
        assert!(q4.filters.iter().any(|f| matches!(f, Filter::After(_))));
        assert!(q4.filters.iter().any(|f| matches!(f, Filter::Before(_))));
        assert_eq!(q4.match_expr.as_deref(), Some("\"error\""));
    }

    #[test]
    fn parser_unknown_keyval_degrades_to_term() {
        let p = QueryParser::new(NOW);
        let q = p.parse("foo:bar");
        assert!(q.filters.is_empty());
        assert_eq!(q.match_expr.as_deref(), Some("\"foobar\""));
    }

    #[test]
    fn parser_tokenize_does_not_hang_on_edge_input() {
        let p = QueryParser::new(NOW);
        for input in ["", "  ", "app:", "\"", "''", "a\"b", "::::", "  :  "] {
            let _ = p.parse(input); // must terminate; no panic
        }
    }

    #[test]
    fn search_finds_ocr_text_ranked() {
        let db = Database::open_in_memory().unwrap();
        // "python traceback" exact phrase (older) vs separate words (newer).
        let phrase_id = insert(&db, "/tmp/p1.png", "shot1.png", 20);
        set_ocr(&db, phrase_id, "running python traceback complete");
        let words_id = insert(&db, "/tmp/p2.png", "shot2.png", 0);
        set_ocr(&db, words_id, "a python file with an unrelated traceback note");
        // Unrelated screenshot mentioning only "python".
        let partial_id = insert(&db, "/tmp/p3.png", "shot3.png", 0);
        set_ocr(&db, partial_id, "python is great");

        let searcher = Searcher::new(&db);
        let out = searcher.search("python traceback", 10, 0).unwrap();
        assert_eq!(out.total, 2, "both docs contain both words");
        // Exact-phrase bonus (+100) must outrank recency/anything else.
        assert_eq!(out.rows[0].row.id, phrase_id);
        assert!(out.rows[0].score > out.rows[1].score);
        assert!(out.rows[0].snippet.is_some());
    }

    #[test]
    fn search_recency_breaks_ties() {
        let db = Database::open_in_memory().unwrap();
        let older = insert(&db, "/tmp/a.png", "a.png", 90);
        set_ocr(&db, older, "docker error occurred");
        let newer = insert(&db, "/tmp/b.png", "b.png", 1);
        set_ocr(&db, newer, "docker error again");

        let out = Searcher::new(&db).search("docker error", 10, 0).unwrap();
        assert_eq!(out.total, 2);
        assert_eq!(out.rows[0].row.id, newer, "equal relevance → newer first");
    }

    #[test]
    fn search_filters() {
        let db = Database::open_in_memory().unwrap();
        let a = insert(&db, "/tmp/Screenshots/a.png", "a.png", 1);
        set_ocr(&db, a, "pricing page screenshot");
        let b = insert(&db, "/tmp/Downloads/b.png", "b.png", 40);
        set_ocr(&db, b, "pricing table image");
        // c has no OCR text at all
        insert(&db, "/tmp/Downloads/c.jpg", "c.jpg", 2);

        let s = Searcher::new(&db);

        // dir filter
        let out = s.search("dir:/tmp/Downloads", 10, 0).unwrap();
        assert_eq!(out.total, 1);
        assert_eq!(out.rows[0].row.filename, "b.png");

        // type filter
        let out = s.search("type:jpg", 10, 0).unwrap();
        assert_eq!(out.total, 1);
        assert_eq!(out.rows[0].row.filename, "c.jpg");

        // has:text (b + a have OCR; c does not)
        let out = s.search("has:text", 10, 0).unwrap();
        assert_eq!(out.total, 2);
        let out = s.search("has:notext", 10, 0).unwrap();
        assert_eq!(out.total, 1);
        assert_eq!(out.rows[0].row.filename, "c.jpg");

        // date filter (only c + a are within last 7 days)
        let out = s.search("after:2026-08-27", 10, 0).unwrap();
        assert_eq!(out.total, 2);

        // tag filter
        add_tag(&db, a, "research");
        let out = s.search("tag:research", 10, 0).unwrap();
        assert_eq!(out.total, 1);
        assert_eq!(out.rows[0].row.id, a);

        // combined: word + filter
        let out = s.search("pricing dir:/tmp/Downloads", 10, 0).unwrap();
        assert_eq!(out.total, 1);
        assert_eq!(out.rows[0].row.filename, "b.png");
    }

    #[test]
    fn search_missing_files_excluded_and_filter_only_queries_work() {
        let db = Database::open_in_memory().unwrap();
        let a = insert(&db, "/tmp/a.png", "a.png", 1);
        db.mark_missing(&[a]);
        insert(&db, "/tmp/b.png", "b.png", 2);

        let out = Searcher::new(&db).search("", 10, 0).unwrap();
        assert_eq!(out.total, 1, "missing files never appear in results");
        assert_eq!(out.rows[0].row.filename, "b.png");
    }

    #[test]
    fn search_duplicates_filter_and_pagination() {
        let db = Database::open_in_memory().unwrap();
        let h = "deadbeef".repeat(8);
        for name in ["a.png", "b.png"] {
            let id = insert(&db, &format!("/tmp/{name}"), name, 1);
            db.conn()
                .execute(
                    "UPDATE screenshots SET content_hash = ?1 WHERE id = ?2",
                    rusqlite::params![h, id],
                )
                .unwrap();
        }
        insert(&db, "/tmp/unique.png", "unique.png", 1);

        let s = Searcher::new(&db);
        let out = s.search("is:duplicate", 10, 0).unwrap();
        assert_eq!(out.total, 2);

        // Pagination via filter-only query
        let p1 = s.search("", 1, 0).unwrap();
        let p2 = s.search("", 10, 1).unwrap();
        assert_eq!(p1.total, 3);
        assert_eq!(p1.rows.len(), 1);
        assert_eq!(p2.rows.len(), 2);
        assert_ne!(p1.rows[0].row.id, p2.rows[0].row.id);
    }
}

// __APPEND__
