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

export interface TagInfo {
  name: string;
  count: number;
}

export interface CollectionInfo {
  id: number;
  name: string;
  kind: string;
  item_count: number;
  created_at: string;
}

export interface TimelineMonth {
  year: number;
  month: number;
  key: string;
  count: number;
}

export interface TimelineDay {
  date: string;
  count: number;
}

export interface DuplicateGroup {
  kind: string;
  key: string;
  items: ScreenshotRow[];
}

export interface Problem {
  id: number;
  path: string | null;
  kind: string;
  message: string;
  created_at: string;
}

export interface ClassifySummary {
  examined: number;
  updated: number;
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
  addTag: (id: number, name: string) => invoke<boolean>("add_tag", { id, name }),
  removeTag: (id: number, name: string) =>
    invoke<boolean>("remove_tag", { id, name }),
  listTags: () => invoke<TagInfo[]>("list_tags"),
  setStarred: (id: number, starred: boolean) =>
    invoke<boolean>("set_starred", { id, starred }),
  setReadLater: (id: number, readLater: boolean) =>
    invoke<boolean>("set_read_later", { id, readLater }),
  setNote: (id: number, note: string) => invoke<boolean>("set_note", { id, note }),
  createCollection: (name: string) =>
    invoke<CollectionInfo>("create_collection", { name }),
  renameCollection: (id: number, name: string) =>
    invoke<boolean>("rename_collection", { id, name }),
  deleteCollection: (id: number) => invoke<boolean>("delete_collection", { id }),
  listCollections: () => invoke<CollectionInfo[]>("list_collections"),
  addToCollection: (collectionId: number, screenshotId: number) =>
    invoke<boolean>("add_to_collection", { collectionId, screenshotId }),
  removeFromCollection: (collectionId: number, screenshotId: number) =>
    invoke<boolean>("remove_from_collection", { collectionId, screenshotId }),
  listCollectionItems: (collectionId: number, limit: number, offset: number) =>
    invoke<ScreenshotRow[]>("list_collection_items", {
      collectionId,
      limit,
      offset,
    }),
  listScreenshotCollections: (id: number) =>
    invoke<CollectionInfo[]>("list_screenshot_collections", { id }),
  timelineMonths: () => invoke<TimelineMonth[]>("timeline_months"),
  timelineDays: (year: number, month: number) =>
    invoke<TimelineDay[]>("timeline_days", { year, month }),
  timelineItems: (date: string, limit: number, offset: number) =>
    invoke<ScreenshotRow[]>("timeline_items", { date, limit, offset }),
  exactDuplicateGroups: () =>
    invoke<DuplicateGroup[]>("exact_duplicate_groups"),
  similarGroups: (maxDistance: number) =>
    invoke<DuplicateGroup[]>("similar_groups", { maxDistance }),
  listProblems: (limit: number) => invoke<Problem[]>("list_problems", { limit }),
  clearProblems: () => invoke<void>("clear_problems"),
  getDataDir: () => invoke<string>("get_data_dir"),
  runClassification: () => invoke<ClassifySummary>("run_classification"),
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
