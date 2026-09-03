// Typed IPC bindings to the Tauri backend.

import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface AppStateDto {
  onboarded: boolean;
  total_screenshots: number;
  problem_count: number;
  indexing: boolean;
}

export interface DirectoryDto {
  id: number;
  path: string;
  enabled: boolean;
}

export interface ScanProgress {
  files_found: number;
  files_processed: number;
  files_indexed: number;
  files_skipped: number;
  files_failed: number;
  current_file: string;
  done: boolean;
}

export interface ScanSummary {
  found: number;
  indexed: number;
  skipped_unchanged: number;
  failed: number;
  cancelled: boolean;
  directories: number;
}

export interface LibraryStats {
  total: number;
  available: number;
  missing: number;
  changed: number;
  pending: number;
  with_ocr: number;
  ocr_failed: number;
  ocr_pending: number;
  ocr_processing: number;
  problem_count: number;
  oldest_ts: number | null;
  newest_ts: number | null;
}

export interface ScreenshotRow {
  id: number;
  path: string;
  filename: string;
  created_ts: number | null;
  width: number | null;
  height: number | null;
  format: string | null;
  status: string;
  ocr_status: string;
  content_hash: string | null;
  phash: string | null;
  starred: boolean;
}

/** Filter values inside a parsed query (serde-tagged enum). */
export interface ParsedQuery {
  raw: string;
  match_expr: string | null;
  phrases: string[];
  adjacency_phrases: string[];
  filters: Array<Record<string, unknown>>;
}

export interface SearchRow extends ScreenshotRow {
  /** Highlighted OCR snippet ([ ] marks the matched words), if any. */
  snippet: string | null;
  /** Deterministic relevance score (higher = better). */
  score: number;
}

export interface SearchOutcome {
  total: number;
  rows: SearchRow[];
  parsed: ParsedQuery;
}

export interface ScreenshotDetail {
  id: number;
  path: string;
  filename: string;
  created_ts: number | null;
  modified_ts: number | null;
  width: number | null;
  height: number | null;
  format: string | null;
  content_hash: string | null;
  phash: string | null;
  status: string;
  ocr_status: string;
  starred: boolean;
  read_later: boolean;
  note: string;
  app_name: string | null;
  website_domain: string | null;
  url: string | null;
  window_title: string | null;
  category: string | null;
  category_confidence: number | null;
  ocr_text: string | null;
  ocr_confidence: number | null;
  tags: string[];
}

export interface OcrProgress {
  total: number;
  processed: number;
  succeeded: number;
  failed: number;
  skipped_missing: number;
  done: boolean;
}

export interface OcrSummary {
  processed: number;
  succeeded: number;
  failed: number;
  skipped_missing: number;
  cancelled: boolean;
  paused_battery: boolean;
}

export const api = {
  getAppState: () => invoke<AppStateDto>("get_app_state"),
  getDefaultDirectories: () => invoke<string[]>("get_default_directories"),
  listDirectories: () => invoke<DirectoryDto[]>("list_directories"),
  addDirectory: (path: string) => invoke<DirectoryDto>("add_directory", { path }),
  removeDirectory: (id: number) => invoke<void>("remove_directory", { id }),
  pickFolder: () => invoke<string | null>("pick_folder"),
  startScan: () => invoke<void>("start_scan"),
  cancelScan: () => invoke<void>("cancel_scan"),
  getStats: () => invoke<LibraryStats>("get_stats"),
  listScreenshots: (limit: number, offset: number) =>
    invoke<ScreenshotRow[]>("list_screenshots", { limit, offset }),
  getThumbnailPath: (contentHash: string, size: number) =>
    invoke<string | null>("get_thumbnail_path", { contentHash, size }),
  search: (query: string, limit: number, offset: number) =>
    invoke<SearchOutcome>("search", { query, limit, offset }),
  getScreenshot: (id: number) =>
    invoke<ScreenshotDetail | null>("get_screenshot", { id }),
  getSetting: (key: string) => invoke<string | null>("get_setting", { key }),
  setSetting: (key: string, value: string) =>
    invoke<void>("set_setting", { key, value }),
  startOcr: () => invoke<void>("start_ocr"),
  cancelOcr: () => invoke<void>("cancel_ocr"),
  retryOcr: () => invoke<number>("retry_ocr"),
};

/** Resolve a cached thumbnail to a loadable asset URL. */
export async function thumbnailUrl(
  contentHash: string | null,
  size = 512
): Promise<string | null> {
  if (!contentHash) return null;
  const p = await api.getThumbnailPath(contentHash, size);
  return p ? convertFileSrc(p) : null;
}

export function onScanProgress(cb: (p: ScanProgress) => void): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan://progress", (e) => cb(e.payload));
}

export function onScanComplete(cb: (s: ScanSummary) => void): Promise<UnlistenFn> {
  return listen<ScanSummary>("scan://complete", (e) => cb(e.payload));
}

export function onOcrProgress(cb: (p: OcrProgress) => void): Promise<UnlistenFn> {
  return listen<OcrProgress>("ocr://progress", (e) => cb(e.payload));
}

export function onOcrComplete(cb: (s: OcrSummary) => void): Promise<UnlistenFn> {
  return listen<OcrSummary>("ocr://complete", (e) => cb(e.payload));
}
