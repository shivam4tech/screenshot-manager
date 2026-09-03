# Screenshot Memory

> **Take screenshots however you already take them. We remember them for you.**

A local-first, cross-platform (Windows / Linux / macOS) desktop application that
turns your existing screenshot collection into a searchable personal visual
memory — without moving, renaming, or modifying a single file.

## Status — 1.0 ready

Working today:

- **Non-destructive scanning** of user-selected directories (recursive), fully
  resumable — unchanged files are fingerprint-skipped, partial scans resume.
- **Per-file error isolation**: corrupted/unsupported files never stop a scan;
  they're recorded in a Problems list instead.
- **Exact-duplicate detection** via streamed SHA-256 content hashing.
- **Visually-similar groundwork** via 64-bit dHash perceptual hashing
  (+ Hamming distance helper) stored on every record.
- **Thumbnail generation & disk cache** keyed by content hash (survives
  renames/moves), with a decode guard for huge images.
- **SQLite persistence** (WAL, transactional batches, versioned migrations)
  with an FTS5 full-text index kept in sync by triggers.
- **Onboarding UI**: welcome → folder selection (platform-appropriate
  defaults) → live scan progress → "your screenshots are searchable".
- **Library grid** with paged thumbnails and an index-health status bar.
- **OCR** (Sprint 2): fully local text extraction via a Tesseract sidecar —
  background worker pipeline with atomic job claiming, retries, cancellation,
  battery-aware pausing, and results searchable the moment they land.
- **Ranked full-text search** (Sprint 2): weighted bm25 over filename/OCR
  text/tags/notes, exact-phrase + recency boosts, highlighted snippets, and a
  filter syntax (`after:2026-08-01`, `tag:research`, `app:chrome`,
  `type:png`, `has:text`, `is:duplicate`, `"exact phrase"`, `last week`, …).
- **Live file watching** (Sprint 2): new/changed screenshots index themselves
  (with write-stability detection), renames keep their identity via content
  hash, deletions mark records missing without destroying metadata.
- **Detail overlay** (Sprint 2): full-size view with metadata, source path,
  and extracted OCR text.
- **Tags** (Sprint 3): manual tagging per screenshot (sidebar with counts,
  click-to-filter via `tag:name`, free-text matches read the tag index).
- **Collections** (Sprint 3): named sets with create/rename/delete, add/remove
  from the detail view or the grid, browsable from the sidebar, searchable via
  `collection:name`.
- **Flags & notes** (Sprint 3): star/unstar (sidebar entry + `is:starred` /
  `is:unstarred` filters), read-later flag, and a searchable free-text note.
- **Timeline** (Sprint 4): month → day → shots drill-down over capture dates
  (local time), with counts at every level.
- **Duplicate review** (Sprint 4): byte-identical groups (shared content hash)
  plus perceptual clusters (dHash Hamming distance, adjustable threshold) for
  review and bulk organization — star all, tag all, collect all. Files are
  never touched; deletion stays a deliberate act outside the app.
- **Auto-enrichment** (Sprint 5): heuristic app / website / category guesses
  from filenames, paths, and OCR text — applied automatically after scans and
  OCR passes, re-runnable from Settings, never overwriting manual values.
- **System tray** (Sprint 5): Show Library, Scan now, and Quit, plus
  left-click to focus the window.
- **Settings** (Sprint 5): watched-folder management, OCR on/off, Enrich now,
  index-health (per-file problems), and the local data location.

## Installing

- **Easiest — download**: grab the installer for your OS from the
  [Releases page](../../releases) (`.deb` / `.AppImage` on Linux, setup on
  Windows, `.dmg` on macOS) and run it. No build needed.
- **One command — run from source**: `npm run app` checks your toolchain,
  installs OS dependencies automatically (webview libs + Tesseract where a
  package manager exists), installs JS deps, and opens the app. Diagnose
  without changing anything via `npm run app:check`.
- **Prerequisites** (only if you skip the launcher): Rust (stable), Node 20+.
  On Linux, the Tauri system packages (`libwebkit2gtk-4.1-dev`,
  `libgtk-3-dev`, `libayatana-appindicator3-dev` for the tray,
  `librsvg2-dev`, `patchelf`). OCR needs the `tesseract` binary on PATH.

## Architecture

```text
┌─ UI (React + TypeScript) ── Tauri IPC ──┐
│                                          │
├─ src-tauri (thin shell: commands, events, logging)
│
├─ core/ (shotmemory-core — pure Rust, no UI deps)
│    db          SQLite + FTS5, migrations, WAL
│    scanner     resumable, read-only, error-isolating
│    hashing     SHA-256 + dHash perceptual hashing
│    metadata    header-only image probing
│    thumbnails  content-hash-keyed disk cache
│    platform    per-OS default paths & dirs
│
└─ Storage: one SQLite file + thumbnail cache in the app data dir
```

**Safety invariants**

- The scanner only *reads* user files. All file mutation (future
  organize/export features) lives in a separate, explicitly-confirmed module.
- Batched transactions + WAL mean a crash during scanning never corrupts the
  index; partial progress is preserved.
- Logs contain operational events only — never screenshot contents or OCR text.

## Development

Prerequisites: Rust (stable), Node 20+, and on Linux the Tauri system packages
(see Installing above). OCR needs the `tesseract` binary on PATH
(`tesseract-ocr` on Debian/Ubuntu, `brew install tesseract` on macOS) —
without it, everything else still works and text extraction simply stays
disabled.

```bash
npm install

# Run core tests (no GUI deps required)
cargo test -p shotmemory-core

# Frontend typecheck + build
npm run build

# Full desktop app (requires Linux webkit packages)
npm run tauri dev
```

## CI

GitHub Actions builds and tests on Ubuntu, Windows, and macOS runners on every
push to `main` (see `.github/workflows/ci.yml`). Pushing a `v*` tag builds
installers and publishes a Release (see `release.yml`); keep the tag in sync
with the versions in `package.json` and `tauri.conf.json`.
