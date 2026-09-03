//! Heuristic auto-classification: source app, website, and category.
//!
//! No network, no models — just transparent keyword rules over the filename,
//! path, and OCR text. Every guess carries a confidence and is trivially
//! explainable ("app:chrome because the OCR text mentions chrome"). Results
//! fill the `app_name` / `website_domain` / `url` / `category` columns, which
//! the existing search filters (`app:`, `site:`) and FTS triggers pick up
//! automatically. Re-running is idempotent: populated columns are left alone.

use serde::Serialize;

use crate::db::Database;
use crate::error::CoreResult;

/// One classification verdict for a screenshot.
#[derive(Debug, Clone, Default)]
pub struct Classification {
    pub app_name: Option<String>,
    pub website_domain: Option<String>,
    pub url: Option<String>,
    pub category: Option<String>,
    pub confidence: f64,
}

/// Summary of an enrichment pass (returned to the UI).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ClassifySummary {
    pub examined: i64,
    pub updated: i64,
}

/// (keyword, canonical app name). Checked against filename + path + OCR.
const APP_PATTERNS: &[(&str, &str)] = &[
    ("google chrome", "chrome"),
    ("chrome.exe", "chrome"),
    ("chrome", "chrome"),
    ("chromium", "chromium"),
    ("firefox", "firefox"),
    ("safari", "safari"),
    ("microsoft edge", "edge"),
    ("msedge", "edge"),
    ("edge", "edge"),
    ("brave", "brave"),
    ("opera", "opera"),
    ("visual studio code", "vscode"),
    ("vscode", "vscode"),
    ("neovim", "neovim"),
    ("vim", "vim"),
    ("emacs", "emacs"),
    ("intellij", "intellij"),
    ("pycharm", "pycharm"),
    ("sublime", "sublime"),
    ("iterm", "iterm2"),
    ("gnome-terminal", "terminal"),
    ("konsole", "terminal"),
    ("powershell", "powershell"),
    ("windows terminal", "terminal"),
    ("terminal", "terminal"),
    ("slack", "slack"),
    ("discord", "discord"),
    ("telegram", "telegram"),
    ("whatsapp", "whatsapp"),
    ("signal", "signal"),
    ("zoom", "zoom"),
    ("microsoft teams", "teams"),
    ("teams", "teams"),
    ("google meet", "meet"),
    ("webex", "webex"),
    ("thunderbird", "thunderbird"),
    ("outlook", "outlook"),
    ("apple mail", "mail"),
    ("gmail", "gmail"),
    ("github desktop", "github-desktop"),
    ("github", "github"),
    ("gitlab", "gitlab"),
    ("stackoverflow", "stackoverflow"),
    ("youtube", "youtube"),
    ("netflix", "netflix"),
    ("spotify", "spotify"),
    ("steam", "steam"),
    ("notion", "notion"),
    ("obsidian", "obsidian"),
    ("figma", "figma"),
    ("photoshop", "photoshop"),
    ("gimp", "gimp"),
    ("libreoffice", "libreoffice"),
    ("microsoft word", "word"),
    ("google docs", "docs"),
    ("microsoft excel", "excel"),
    ("google sheets", "sheets"),
    ("finder", "finder"),
    ("file explorer", "explorer"),
    ("nautilus", "files"),
    ("dolphin", "dolphin"),
];

/// (category, keywords). Highest hit count wins.
const CATEGORY_KEYWORDS: &[(&str, &[&str])] = &[
    (
        "code",
        &[
            "traceback", "exception", "null pointer", "segfault", "compiler",
            "pull request", "merge conflict", "localhost", "stack overflow",
            "import ", "def ", "fn ", "cargo", "npm", "git ",
        ],
    ),
    (
        "finance",
        &[
            "invoice", "receipt", "payment", "refund", "balance", "transaction",
            "bank", "tax", "payroll", "order total",
        ],
    ),
    (
        "communication",
        &[
            "inbox", "re:", "fwd:", "meeting", "slack", "discord", "telegram",
            "message", "unread", "attachment",
        ],
    ),
    (
        "docs",
        &[
            "resume", "curriculum vitae", "contract", "report", "memo",
            "minutes", "agenda", "proposal", "manuscript",
        ],
    ),
    (
        "media",
        &[
            "youtube", "netflix", "episode", "season", "playlist", "spotify",
            "video", "subtitle",
        ],
    ),
    (
        "social",
        &[
            "tweet", "retweet", "follower", "reddit", "upvote", "instagram",
            "facebook", "linkedin", "like",
        ],
    ),
    (
        "shopping",
        &[
            "cart", "checkout", "order", "shipping", "discount", "coupon",
            "price", "amazon",
        ],
    ),
];

/// Classify one record from its filename, path, and OCR text.
pub fn classify_record(filename: &str, path: &str, ocr_text: Option<&str>) -> Classification {
    let mut out = Classification::default();
    let file_hit = format!("{filename}\n{path}").to_lowercase();
    let ocr = ocr_text.unwrap_or("").to_lowercase();

    for (keyword, app) in APP_PATTERNS {
        if file_hit.contains(keyword) || ocr.contains(keyword) {
            out.app_name = Some((*app).to_string());
            break;
        }
    }

    if let Some((url, domain)) = extract_url(ocr_text.unwrap_or("")) {
        out.url = Some(url);
        out.website_domain = Some(domain);
    } else if let Some(domain) = extract_www_domain(ocr_text.unwrap_or("")) {
        out.website_domain = Some(domain);
    }

    let haystack = format!("{file_hit}\n{ocr}");
    let mut best: Option<(&str, usize)> = None;
    for (category, keywords) in CATEGORY_KEYWORDS {
        let hits = keywords.iter().filter(|k| haystack.contains(**k)).count();
        if hits > 0 && best.map_or(true, |(_, b)| hits > b) {
            best = Some((category, hits));
        }
    }
    if let Some((category, hits)) = best {
        out.category = Some(category.to_string());
        out.confidence = (0.55 + 0.1 * hits as f64).min(0.9);
    } else if out.app_name.is_some() || out.website_domain.is_some() {
        out.confidence = 0.5;
    }
    out
}

/// First http(s) URL in the text, plus its bare domain.
fn extract_url(text: &str) -> Option<(String, String)> {
    for marker in ["https://", "http://"] {
        if let Some(start) = text.find(marker) {
            let rest = &text[start + marker.len()..];
            let end = rest
                .find(|c: char| c.is_whitespace() || "<>\"'()[]{}".contains(c))
                .unwrap_or(rest.len());
            let host_path = &rest[..end];
            if host_path.is_empty() {
                continue;
            }
            let host = host_path.split('/').next().unwrap_or("");
            let domain = host
                .strip_prefix("www.")
                .unwrap_or(host)
                .to_ascii_lowercase();
            if domain.contains('.') {
                let url = format!("{marker}{host_path}");
                return Some((url, domain));
            }
        }
    }
    None
}

/// Bare "www.example.com" mention without a scheme.
fn extract_www_domain(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let mut search = lower.as_str();
    while let Some(pos) = search.find("www.") {
        let rest = &search[pos + 4..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
            .unwrap_or(rest.len());
        let domain = format!("www.{}", &rest[..end]);
        if domain.contains('.') && end > 0 && rest[..end].contains('.') {
            return Some(domain);
        }
        search = &rest[end.min(rest.len())..];
        if search.is_empty() {
            break;
        }
    }
    None
}

/// Enrich every record with missing app/domain/url/category. Populated
/// columns are never overwritten, so manual corrections and re-runs are safe.
pub fn apply_classification(db: &Database) -> CoreResult<ClassifySummary> {
    let mut stmt = db.conn().prepare(
        "SELECT s.id, s.filename, s.path, s.app_name, s.website_domain, s.url,
                s.category, o.text
         FROM screenshots s
         LEFT JOIN ocr_text o ON o.screenshot_id = s.id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<Result<
            Vec<(i64, String, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)>,
            _,
        >>()?;
    drop(stmt);

    let mut summary = ClassifySummary::default();
    let tx = db.conn().unchecked_transaction()?;
    for (id, filename, path, app, domain, url, category, ocr) in &rows {
        summary.examined += 1;
        if app.is_some() && domain.is_some() && url.is_some() && category.is_some() {
            continue;
        }
        let verdict = classify_record(filename, path, ocr.as_deref());
        let new_app = app.clone().or(verdict.app_name);
        let new_domain = domain.clone().or(verdict.website_domain);
        let new_url = url.clone().or(verdict.url);
        let (new_category, new_conf) = match (&category, verdict.category) {
            (Some(_), _) => (category.clone(), None),
            (None, Some(c)) => (Some(c), Some(verdict.confidence)),
            (None, None) => (None, None),
        };
        if new_app != *app || new_domain != *domain || new_url != *url || new_category != *category {
            tx.execute(
                "UPDATE screenshots
                 SET app_name = ?2, website_domain = ?3, url = ?4,
                     category = ?5, category_confidence = COALESCE(?6, category_confidence)
                 WHERE id = ?1",
                rusqlite::params![id, new_app, new_domain, new_url, new_category, new_conf],
            )?;
            summary.updated += 1;
        }
    }
    tx.commit()?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScreenshot;

    #[test]
    fn app_guessed_from_filename_and_path() {
        let c = classify_record("Screenshot-chrome-error.png", "/home/u/Pics", None);
        assert_eq!(c.app_name.as_deref(), Some("chrome"));
        let c = classify_record("shot.png", "/home/u/.config/slack/logs", None);
        assert_eq!(c.app_name.as_deref(), Some("slack"));
        let c = classify_record("Screenshot 2026.png", "/tmp", None);
        assert_eq!(c.app_name, None);
    }

    #[test]
    fn url_and_domain_extracted_from_ocr() {
        let c = classify_record(
            "a.png",
            "/tmp",
            Some("see https://github.com/anomalyco/opencode/pulls (open pull request) for details"),
        );
        assert_eq!(c.url.as_deref(), Some("https://github.com/anomalyco/opencode/pulls"));
        assert_eq!(c.website_domain.as_deref(), Some("github.com"));
        // App keyword in the same text is picked up too.
        assert_eq!(c.app_name.as_deref(), Some("github"));
        assert_eq!(c.category.as_deref(), Some("code"));
    }

    #[test]
    fn www_mention_yields_domain_only() {
        let c = classify_record("a.png", "/tmp", Some("visit www.example.com today"));
        assert_eq!(c.url, None);
        assert_eq!(c.website_domain.as_deref(), Some("www.example.com"));
    }

    #[test]
    fn apply_fills_gaps_and_skips_complete_rows() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .insert_screenshot(&NewScreenshot {
                path: "/tmp/chrome-invoice.png".into(),
                filename: "chrome-invoice.png".into(),
                ..Default::default()
            })
            .unwrap();
        db.save_ocr_text(id, "your invoice total https://pay.example.com/x", None, "t")
            .unwrap();

        let s = apply_classification(&db).unwrap();
        assert_eq!(s.examined, 1);
        assert_eq!(s.updated, 1);
        let d = db.get_screenshot_detail(id).unwrap().unwrap();
        assert_eq!(d.app_name.as_deref(), Some("chrome"));
        assert_eq!(d.website_domain.as_deref(), Some("pay.example.com"));
        assert_eq!(d.category.as_deref(), Some("finance"));

        // Second run: nothing to do.
        let s = apply_classification(&db).unwrap();
        assert_eq!(s.updated, 0);

        // Manual values are never overwritten.
        db.conn()
            .execute(
                "UPDATE screenshots SET app_name = 'firefox' WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        apply_classification(&db).unwrap();
        let d = db.get_screenshot_detail(id).unwrap().unwrap();
        assert_eq!(d.app_name.as_deref(), Some("firefox"));
    }
}
