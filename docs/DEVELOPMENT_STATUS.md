# Development cycle audit — 2026-10-17

The saved Sprint 2 plan and the Sprint 1–5 features described in the README are
implemented. The later cleanup, rename, suggestion review, and cull workflows are
also present. No open GitHub issues were listed during this audit. The remaining
work was integration defects, failing platform checks, regression coverage, and
outdated documentation; this cycle addresses those items.

| Scope | Implementation and evidence |
| --- | --- |
| Sprint 1: indexing | Scanner, hashes, thumbnails, SQLite/FTS, onboarding, and grid; core tests and scan/search/organize integration smoke test. |
| Sprint 2: findability | OCR, ranked search, filters, detail, watcher, commands, and UI events; watcher reconfiguration/rename and OCR queue-shrink regressions now covered. |
| Sprint 3: organization | Tags, collections, stars, read-later, notes; persistence and search tests. |
| Sprint 4: review | Timeline, exact/similar duplicates, bulk organization; insight and search tests. |
| Sprint 5: desktop delivery | Enrichment, tray, settings, launcher, and installer workflow; Linux native startup and Debian bundle verified. |
| Later development | Cleanup analysis and deleted memories, safe rename with journal, local suggestions, Review, bursts, infinite grids, and cull; core tests plus new frontend cull tests. |

## Fixes completed in this cycle

- Rebuild the live watcher when folders change, preserving reconfiguration
  requests received during setup. Process old rename paths first so tags and
  screenshot identity survive renames in either alphabetical direction. Resolve
  ambiguous macOS rename events using final file existence before ordering.
- Start enabled OCR after watcher changes; notify the library and refresh active
  searches after file events, scan completion, and OCR completion. Recheck queued
  work when OCR exits, and apply the Settings OCR toggle immediately.
- Finish the OCR coordinator when workers exit, even if files disappeared after
  the initial queue count. Prevent a stalled progress indicator in that case.
- Match native Windows folder separators and literal SQL wildcard characters in
  directory searches. Make path test expectations platform independent.
- Compile macOS without the Linux/Windows-only trash restore API. Hide unsupported
  undo actions and explain Finder recovery in cull mode. Normalize Windows
  DOS short names and verbatim/case path variants when locating an item in
  the Recycle Bin.
- Lock cull navigation, duplicate delete shortcuts, and exit while a file action
  is running. Keep failed deletions visible; report detail/review failures.
- Show OCR-unavailable errors while pending jobs exist, and clean up event
  subscriptions that register after a component unmounts.
- Add frontend regression tests and a single verification command. CI and release
  packaging now run regression checks. Apply the compatible npm security patch.
- Archive the original saved plan and update product/release text to describe
  explicit rename/trash behavior accurately.

## Verification

On Linux:

- `npm run verify`: 7 frontend tests, production frontend build/typecheck,
  99 core unit tests, and 1 integration smoke test passed.
- `cargo check --workspace --locked --quiet`: passed.
- `npm run tauri -- build --debug --bundles deb`: built the desktop executable
  and `target/debug/bundle/deb/Screenshot Memory_1.0.0_amd64.deb`.
- Native executable launched under Xvfb with isolated XDG data/cache directories,
  stayed running for 10 seconds, initialized SQLite, and passed `integrity_check`.
  This is a startup smoke test, not a full interactive desktop acceptance test.
- `npm audit`: zero reported vulnerabilities after the compatible patch.
- `git diff --check`: passed.

The CI workflow runs frontend tests/build and Rust workspace tests/checks on
Ubuntu, Windows, and macOS. Installer publication remains the existing `v*` tag
workflow; pushing development changes to `main` does not publish a release.

## Platform boundaries

In-app Trash undo is available on Linux and Windows. The trash dependency does
not provide restoration on macOS: use **Put Back** in Finder's Trash, then scan
again. Trash itself remains recoverable on all supported desktop platforms.

Tesseract must be installed separately and available on PATH. The app remains
usable without OCR; the status bar explains an unavailable engine. Installers
remain unsigned. Native macOS/Windows GUI interaction and signed installer
validation require those environments; automated CI covers their builds/tests.
