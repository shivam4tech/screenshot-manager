//! End-to-end smoke test: real PNG files on disk flow through the whole
//! engine — scan → search → tags/collections → classification → timeline →
//! duplicate groups — exactly as the desktop app drives it (minus the GUI).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use shotmemory_core::db::Database;
use shotmemory_core::scanner::Scanner;
use shotmemory_core::search::Searcher;

fn paint_png(path: &Path, w: u32, h: u32, seed: u8) {
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let v = x.wrapping_add(y).wrapping_add(seed as u32) as u8;
        *px = image::Rgb([v, 128, 200u8.wrapping_sub(seed)]);
    }
    image::DynamicImage::ImageRgb8(img).save(path).unwrap();
}

#[test]
fn app_smoke_scan_search_organize_insights() {
    let data_dir = tempfile::tempdir().unwrap();
    let cache_dir = tempfile::tempdir().unwrap();
    let src = data_dir.path().join("Screenshots");
    std::fs::create_dir_all(&src).unwrap();

    // Two byte-identical files (exact-dupe pair) + one distinct shot.
    paint_png(&src.join("chrome-invoice-march.png"), 300, 200, 7);
    std::fs::copy(
        src.join("chrome-invoice-march.png"),
        src.join("chrome-invoice-march-copy.png"),
    )
    .unwrap();
    paint_png(&src.join("terminal-notes.png"), 320, 200, 99);

    let db = Database::open_in_memory().unwrap();
    db.add_directory(&src).unwrap();
    let scanner = Scanner::new(&db, cache_dir.path());
    let summary = scanner
        .scan_directories(&[src], &AtomicBool::new(false), &mut |_| {})
        .unwrap();
    assert_eq!(summary.indexed, 3, "all three shots indexed");
    assert_eq!(db.library_stats().unwrap().available, 3);

    // Search by filename.
    let searcher = Searcher::new(&db);
    let out = searcher.search("chrome", 10, 0).unwrap();
    assert_eq!(out.total, 2);
    let out = searcher.search("is:duplicate", 10, 0).unwrap();
    assert_eq!(out.total, 2, "the identical pair is flagged");

    // Organize: tag + collection flow through search filters.
    let first = out.rows[0].row.id;
    assert!(db.add_tag(first, "finance").unwrap());
    assert_eq!(searcher.search("tag:finance", 10, 0).unwrap().total, 1);
    let col = db.create_collection("Work").unwrap();
    assert!(db.add_to_collection(col.id, first).unwrap());
    assert_eq!(
        searcher.search("collection:work", 10, 0).unwrap().total,
        1
    );
    assert!(db.set_starred(first, true).unwrap());
    assert_eq!(searcher.search("is:starred", 10, 0).unwrap().total, 1);

    // Classification fills app guesses from filenames.
    let cs = shotmemory_core::classify::apply_classification(&db).unwrap();
    assert_eq!(cs.examined, 3);
    assert!(cs.updated >= 2, "chrome files classified, got {cs:?}");
    let d = db.get_screenshot_detail(first).unwrap().unwrap();
    assert_eq!(d.app_name.as_deref(), Some("chrome"));

    // Timeline covers everything scanned just now.
    let months = shotmemory_core::insights::timeline_months(&db).unwrap();
    let total: i64 = months.iter().map(|m| m.count).sum();
    assert_eq!(total, 3);
    let top = months.iter().max_by_key(|m| m.count).unwrap();
    let days = shotmemory_core::insights::timeline_days(&db, top.year, top.month).unwrap();
    assert!(!days.is_empty());

    // Duplicate review surfaces the identical pair.
    let groups = shotmemory_core::insights::exact_duplicate_groups(&db).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].items.len(), 2);
    // Similar clustering runs without error (gradient shots may or may not cluster).
    let _ = shotmemory_core::insights::similar_groups(&db, 8).unwrap();
}
