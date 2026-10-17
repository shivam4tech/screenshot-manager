# Sprint 2 — "Make it findable" (OCR + full-text search + live watcher): completion plan

> Archived plan, audited 2026-10-17: implementation complete, including the
> watcher/OCR integration fixes. See [current development status](../../docs/DEVELOPMENT_STATUS.md)
> for verification and platform limitations. The text below records the original scope.

Sprint 1 (committed `0710f55`) is done: scanner, hashing, metadata, thumbnails, DB/FTS5, onboarding, library grid. Sprint 2 core work already exists on disk but is unwired and broken. This plan finishes it.

## 1. Fix the core bug
- `core/src/search.rs:580-590`: remove the duplicated `where_sql` / `args.push(limit/offset)` block (bounds limit+offset twice → parameter-count mismatch; every search errors, core tests fail). Remove the stray `// __APPEND__` marker at `search.rs:889`.
- Verify: `cargo test -p shotmemory-core` green.

## 2. Implement the missing Tauri commands (`src-tauri/src/commands.rs`)
`main.rs` already registers these — they just need to exist:
- `search(query, limit, offset)` → `SearchOutcome` (snippet + score per row), via `state.db`
- `get_screenshot(id)` → `ScreenshotDetail` (core `db.get_screenshot_detail`)
- `get_setting(key)` / `set_setting(key, value)` — generic settings passthrough
- `start_ocr` — spawn `OcrPipeline` on a background thread with its own DB connection, engine from `TesseractEngine::discover()`, emit `ocr://progress` + `ocr://complete` events; guard with the existing `ocr_running` flag (same pattern as `start_scan`). If tesseract isn't found, return a clear "OCR unavailable — install tesseract" state instead of erroring.
- `cancel_ocr` — set the shared cancel flag
- `retry_ocr` — re-queue failed jobs (`db.retry_failed_ocr`) then kick a run
- `spawn_ocr_if_enabled(app)` — helper already called in `setup()`; also call it after each scan completes so OCR starts automatically once indexing finishes
- `add_directory` / `remove_directory` — call `state.watcher.reconfigure()` so new folders are watched live without a restart

## 3. Frontend wiring
- `src/api.ts`: add types (`SearchRow`, `SearchOutcome`, `ScreenshotDetail`, extend `LibraryStats` with `ocr_pending` / `ocr_processing`) and bindings for search / getScreenshot / startOcr / cancelOcr / retryOcr / settings; add `ocr://progress` / `ocr://complete` listeners
- `src/components/Library.tsx`: enable the search bar; debounced live search; reuse the thumbnail grid for results; show the OCR snippet under each hit; result count + clear button; query-syntax hint (e.g. `after:2026-08-01 tag:… is:duplicate`) in placeholder/tooltip
- `src/components/StatusBar.tsx`: OCR progress (processed/total + cancel), pending/failed counts, retry button when failures exist, "watching live" indicator
- Minimal detail panel: click a result → overlay with metadata + full OCR text (uses `get_screenshot`; full viewer/timeline stays in Sprint 3)

## 4. Docs
- README: move OCR / search / watcher from "Coming in later sprints" to "Working today"; add tesseract prerequisite note for OCR.

## 5. Verification
- `cargo test -p shotmemory-core` and `cargo check --workspace` (workspace must compile again — CI runs this)
- `npm run build` (typecheck + bundle)
- Manual smoke via `npm run tauri dev`: scan a folder → OCR progresses → search by extracted text with snippets → add folder → drop a file in → auto-indexed → delete → marked missing

## Defaults baked in (flag if you want them changed)
- OCR auto-starts after a scan completes, single worker, gentle defaults — matches `spawn_ocr_if_enabled` already wired in `main.rs`
- Sprints 3–5 remain as README lists: timeline, collections & tags, duplicate manager → classification, system tray → packaging
