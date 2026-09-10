import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  thumbnailUrl,
  type CleanupItem,
  type CleanupOverview,
  type DeleteFailure,
  type DeletedMemory,
} from "../api";
import { BulkBar, useSelection } from "./bulk";
import { ScreenshotCard } from "./ScreenshotCard";
import { useInfiniteLoader } from "./scroll";
import { Button, Dropdown, EmptyState, formatBytes, useConfirm, type ToastAction } from "./ui";
import { Icons } from "./icons";

const PAGE_SIZE = 60;
const AGE_OPTIONS = [
  { days: 30, label: "30 days" },
  { days: 90, label: "90 days" },
  { days: 180, label: "6 months" },
  { days: 365, label: "1 year" },
  { days: 730, label: "2 years" },
];
const SIZE_OPTIONS = [
  { bytes: 2_000_000, label: "> 2 MB" },
  { bytes: 5_000_000, label: "> 5 MB" },
  { bytes: 10_000_000, label: "> 10 MB" },
];

type ReviewCategory = "old" | "large" | "notext";

function parseNames(json: string): string[] {
  try {
    const v: unknown = JSON.parse(json);
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function utcToLocal(s: string): string {
  const d = new Date(s.replace(" ", "T") + "Z");
  return isNaN(d.getTime()) ? s : d.toLocaleString();
}

/** Read-only viewer for one deleted-memory record. Never touches files. */
function MemoryViewer({ memory, onClose }: { memory: DeletedMemory; onClose: () => void }) {
  const [thumb, setThumb] = useState<string | null>(null);
  useEffect(() => {
    if (memory.keep_thumbnail && memory.content_hash) {
      thumbnailUrl(memory.content_hash, 512).then(setThumb).catch(() => {});
    }
  }, [memory]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const tags = parseNames(memory.tags);
  const collections = parseNames(memory.collections);
  return (
    <div className="detail-backdrop" onClick={onClose} role="dialog" aria-modal="true" aria-label="Deleted record">
      <div className="memory-panel" onClick={(e) => e.stopPropagation()}>
        <div className="detail-head">
          <div style={{ minWidth: 0 }}>
            <h3 title={memory.filename}>{memory.filename}</h3>
            <p className="detail-date">Deleted {utcToLocal(memory.deleted_at)} · original file unavailable</p>
          </div>
          <button className="iconbtn" onClick={onClose} aria-label="Close">✕</button>
        </div>
        {thumb ? (
          <img className="memory-thumb" src={thumb} alt="" />
        ) : (
          <p className="muted small">No preview retained for this record.</p>
        )}
        <dl className="kv">
          <dt>Original path</dt>
          <dd className="mono path-trunc" title={memory.path}>{memory.path}</dd>
          <dt>Captured</dt>
          <dd>{memory.created_ts ? new Date(memory.created_ts * 1000).toLocaleString() : "—"}</dd>
          <dt>Size</dt>
          <dd>{formatBytes(memory.size)}</dd>
          {memory.width && memory.height ? (<><dt>Dimensions</dt><dd>{memory.width} × {memory.height} px</dd></>) : null}
          {memory.format ? (<><dt>Format</dt><dd>{memory.format}</dd></>) : null}
          {memory.app_name || memory.category ? (<><dt>Source</dt><dd>{[memory.app_name, memory.category].filter(Boolean).join(" · ")}</dd></>) : null}
          {memory.note ? (<><dt>Note</dt><dd>{memory.note}</dd></>) : null}
        </dl>
        {(tags.length > 0 || collections.length > 0) && (
          <div className="tag-row">
            {tags.map((t) => (
              <span className="tag-chip" key={`t-${t}`}>{t}</span>
            ))}
            {collections.map((c) => (
              <span className="tag-chip" key={`c-${c}`}>▦ {c}</span>
            ))}
          </div>
        )}
        <span className="insp-label">OCR text</span>
        {memory.ocr_text.trim() ? (
          <pre className="ocr-box">{memory.ocr_text}</pre>
        ) : (
          <span className="muted small">No text was extracted.</span>
        )}
      </div>
    </div>
  );
}

/**
 * Cleanup dashboard: indexed-metadata analysis plus reviewed trash with
 * optional searchable deletion records. Exact/near/burst review reuses the
 * existing Duplicates/Bursts views; old/large/notext review inline grids
 * with the shared selection toolbar.
 */
export default function Cleanup({
  onOpenDetail,
  onNavigate,
  onNotify,
}: {
  onOpenDetail: (id: number) => void;
  onNavigate: (view: "duplicates" | "bursts") => void;
  onNotify: (msg: string, action?: ToastAction) => void;
}) {
  const [overview, setOverview] = useState<CleanupOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [review, setReview] = useState<ReviewCategory | null>(null);
  const [ageDays, setAgeDays] = useState(365);
  const [minBytes, setMinBytes] = useState(5_000_000);
  const [sort, setSort] = useState("oldest");
  const [items, setItems] = useState<CleanupItem[]>([]);
  const [itemsTotal, setItemsTotal] = useState(0);
  const [itemsBytes, setItemsBytes] = useState(0);
  const [itemsHasMore, setItemsHasMore] = useState(false);
  const [itemsLoading, setItemsLoading] = useState(false);
  const [thumbs, setThumbs] = useState<Map<number, string>>(new Map());
  const [failures, setFailures] = useState<DeleteFailure[]>([]);
  const sel = useSelection();
  const [selectingAll, setSelectingAll] = useState(false);
  const [selBytes, setSelBytes] = useState<number | null>(null);
  const trashHotkeyRef = useRef<(() => void) | null>(null);
  const { confirm, confirmNode } = useConfirm();

  const [memEnabled, setMemEnabled] = useState(false);
  const [memories, setMemories] = useState<DeletedMemory[]>([]);
  const [memTotal, setMemTotal] = useState(0);
  const [memQuery, setMemQuery] = useState("");
  const [memViewing, setMemViewing] = useState<DeletedMemory | null>(null);

  const refreshOverview = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const o = await api.cleanupOverview();
      setOverview(o);
      setAgeDays(o.old_age_days);
      setMinBytes(o.large_min_bytes);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  const loadMemories = useCallback(async (query: string) => {
    try {
      const enabled = await api.getSetting("keep_deleted_memory");
      setMemEnabled(enabled === "1");
      const page = await api.listDeletedMemories(query, 50, 0);
      setMemTotal(page.total);
      setMemories(page.rows);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refreshOverview();
    loadMemories("");
  }, [refreshOverview, loadMemories]);

  useEffect(() => {
    const t = setTimeout(() => loadMemories(memQuery).catch(() => {}), 300);
    return () => clearTimeout(t);
  }, [memQuery, loadMemories]);

  const resolveThumbs = useCallback(async (rows: CleanupItem[]) => {
    const fresh = new Map<number, string>();
    for (const row of rows) {
      const url = await thumbnailUrl(row.content_hash, 512);
      if (url) fresh.set(row.id, url);
    }
    setThumbs((m) => new Map([...m, ...fresh]));
  }, []);

  const loadItems = useCallback(
    async (category: ReviewCategory, offset: number) => {
      setItemsLoading(true);
      try {
        const page = await api.cleanupItems(
          category,
          category === "old" ? ageDays : 0,
          category === "large" ? minBytes : 0,
          sort,
          PAGE_SIZE,
          offset
        );
        setItemsTotal(page.total);
        setItemsBytes(page.bytes);
        setItemsHasMore(page.rows.length === PAGE_SIZE);
        setItems((r) => (offset === 0 ? page.rows : [...r, ...page.rows]));
        void resolveThumbs(page.rows);
      } catch (e) {
        setError(String(e));
      } finally {
        setItemsLoading(false);
      }
    },
    [ageDays, minBytes, sort, resolveThumbs]
  );

  const openReview = (category: ReviewCategory) => {
    setReview(category);
    setItems([]);
    sel.clear();
    setFailures([]);
    setSort(category === "large" ? "largest" : category === "old" ? "oldest" : "newest");
  };

  useEffect(() => {
    if (review) loadItems(review, 0).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [review, ageDays, minBytes, sort]);

  const loadMore = useCallback(() => {
    if (review) loadItems(review, items.length).catch(() => {});
  }, [review, items.length, loadItems]);
  const sentinel = useInfiniteLoader(review !== null && itemsHasMore, itemsLoading, loadMore);

  // Live byte estimate for the current selection (pre-confirm wording).
  useEffect(() => {
    if (sel.selected.size === 0) {
      setSelBytes(null);
      return;
    }
    let alive = true;
    api
      .cleanupSizes([...sel.selected])
      .then((n) => alive && setSelBytes(n))
      .catch(() => alive && setSelBytes(null));
    return () => {
      alive = false;
    };
  }, [sel.selected]);

  const loadedSelectedBytes = items
    .filter((r) => sel.selected.has(r.id))
    .reduce((n, r) => n + r.size, 0);
  const estBytes = selBytes ?? loadedSelectedBytes;

  const selectAllInReview = async () => {
    if (!review) return;
    setSelectingAll(true);
    try {
      const ids = await api.cleanupCategoryIds(
        review,
        review === "old" ? ageDays : 0,
        review === "large" ? minBytes : 0
      );
      sel.selectAll(ids);
    } catch (e) {
      setError(String(e));
    } finally {
      setSelectingAll(false);
    }
  };

  const undoTrash = async (ids: number[]) => {
    try {
      const s = await api.restoreScreenshots(ids);
      const ok = ids.filter((id) => !s.failed.some((f) => f.id === id));
      refreshOverview();
      if (review) loadItems(review, 0).catch(() => {});
      if (ok.length > 0) sel.selectAll(ok);
      onNotify(
        ok.length === ids.length
          ? `Restored ${ok.length} screenshot${ok.length === 1 ? "" : "s"}.`
          : `Restored ${ok.length} of ${ids.length}.`
      );
    } catch (e) {
      setError(String(e));
    }
  };

  const trashSelection = async (ids: number[]) => {
    const s = await api.cleanupTrash(ids);
    const gone = ids.filter((id) => !s.failed.some((f) => f.id === id));
    setFailures(s.failed);
    const bits = [`${s.trashed} trashed`];
    if (s.already_missing > 0) bits.push(`${s.already_missing} already gone`);
    if (s.failed.length > 0) bits.push(`${s.failed.length} failed`);
    let note = bits.join(", ") + ".";
    if (s.memories_retained > 0) {
      note += ` ${s.memories_retained} searchable record${s.memories_retained === 1 ? "" : "s"} retained.`;
    }
    return { gone, note };
  };

  const afterBulk = (removedIds: number[]) => {
    if (removedIds.length > 0) {
      const gone = new Set(removedIds);
      const remaining = items.filter((r) => !gone.has(r.id));
      setItems(remaining);
      setItemsTotal((t) => Math.max(0, t - removedIds.length));
      setItemsBytes(remaining.reduce((n, r) => n + r.size, 0));
    }
    sel.clear();
    refreshOverview();
    loadMemories(memQuery).catch(() => {});
    if (removedIds.length > 0) {
      onNotify(`${removedIds.length} screenshot${removedIds.length === 1 ? "" : "s"} moved to Trash.`, {
        label: "Undo",
        fn: () => void undoTrash(removedIds),
      });
    }
  };

  const deleteMemory = async (m: DeletedMemory) => {
    const ok = await confirm({
      title: `Permanently delete this record?`,
      body: `“${m.filename}”\n\nThe metadata record is removed forever. This cannot be undone.`,
      confirmLabel: "Delete record",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.deleteDeletedMemory(m.id);
      loadMemories(memQuery).catch(() => {});
    } catch (e) {
      setError(String(e));
    }
  };

  const clearMemories = async () => {
    const ok = await confirm({
      title: `Delete all ${memTotal} records?`,
      body: "Every searchable deletion record is removed forever. This cannot be undone.",
      confirmLabel: "Delete all",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.clearDeletedMemories();
      loadMemories(memQuery).catch(() => {});
    } catch (e) {
      setError(String(e));
    }
  };

  const analyzedLabel = (() => {
    if (!overview) return "";
    const d = new Date(overview.analyzed_at.replace(" ", "T") + "Z");
    return isNaN(d.getTime()) ? "" : d.toLocaleString();
  })();

  if (review) {
    const ageLabel = AGE_OPTIONS.find((o) => o.days === ageDays)?.label ?? "";
    const sizeLabel = SIZE_OPTIONS.find((o) => o.bytes === minBytes)?.label ?? "";
    const title =
      review === "old" ? `Older than ${ageLabel}`
      : review === "large" ? `Larger than ${sizeLabel.replace("> ", "")}`
      : "No detected text";
    const sub = `${itemsTotal.toLocaleString()} screenshots · ${formatBytes(itemsBytes)}`;
    return (
      <div className="cleanup">
        <div className="page-head">
          <div>
            <button className="link-btn" onClick={() => { setReview(null); sel.clear(); setFailures([]); }}>
              ← Cleanup
            </button>
            <h1 className="page-title">{title}</h1>
            <p className="page-sub">{sub}</p>
          </div>
        </div>
        <div className="toolbar-row" role="toolbar" aria-label="Review options">
          {review === "old" && (
            <Dropdown
              ariaLabel="Age threshold"
              prefix="Older than:"
              value={String(ageDays)}
              onChange={(v) => setAgeDays(Number(v))}
              options={AGE_OPTIONS.map((o) => ({ value: String(o.days), label: o.label }))}
            />
          )}
          {review === "large" && (
            <Dropdown
              ariaLabel="Size threshold"
              prefix="Size:"
              value={String(minBytes)}
              onChange={(v) => setMinBytes(Number(v))}
              options={SIZE_OPTIONS.map((o) => ({ value: String(o.bytes), label: o.label }))}
            />
          )}
          <Dropdown
            ariaLabel="Sort order"
            prefix="Sort:"
            value={sort}
            onChange={setSort}
            options={[
              { value: "oldest", label: "Oldest first" },
              { value: "newest", label: "Newest first" },
              { value: "largest", label: "Largest first" },
            ]}
          />
        </div>
        {error && <p className="error">{error}</p>}
        <div className="selbar">
          <BulkBar
            ids={[...sel.selected]}
            collections={[]}
            onDone={afterBulk}
            onError={setError}
            selectAllLabel={`Select all ${itemsTotal.toLocaleString()}`}
            selectingAll={selectingAll}
            onSelectAll={() => void selectAllInReview()}
            onCancel={() => sel.clear()}
            trashHotkeyRef={trashHotkeyRef}
            trashIds={trashSelection}
            selectionBytes={sel.selected.size > 0 ? estBytes : null}
            confirmTitle={`Move ${sel.selected.size} screenshot${sel.selected.size === 1 ? "" : "s"} to Trash?`}
            confirmBody={`${formatBytes(estBytes)} selected\n\nThese files will be moved using the operating system's Trash / Recycle Bin.`}
          />
        </div>
        {failures.length > 0 && (
          <div className="failures" role="alert">
            <div className="failures-head">
              <span>{failures.length} file{failures.length === 1 ? "" : "s"} could not be moved</span>
              <button className="link-btn" onClick={() => setFailures([])}>Dismiss</button>
            </div>
            <ul>
              {failures.map((f) => (
                <li key={f.id}>
                  <span className="mono small">{f.path ?? `id ${f.id}`}</span>
                  <span className="muted small"> — {f.message}</span>
                </li>
              ))}
            </ul>
          </div>
        )}
        {items.length === 0 && !itemsLoading ? (
          <EmptyState
            title="Nothing to review here"
            body="No screenshots match this category right now."
            action={<Button onClick={() => setReview(null)}>Back to Cleanup</Button>}
          />
        ) : (
          <>
            <div className="shot-grid">
              {items.map((r) => (
                <ScreenshotCard
                  key={r.id}
                  row={{ ...r, status: "available" }}
                  thumbUrl={thumbs.get(r.id)}
                  selected={sel.selected.has(r.id)}
                  onOpen={onOpenDetail}
                  onToggleSelect={(id) => sel.toggle(id)}
                />
              ))}
            </div>
            <div ref={sentinel} className="scroll-sentinel" aria-hidden="true">
              {itemsLoading ? "Loading…" : !itemsHasMore && items.length > 0 ? "End." : ""}
            </div>
          </>
        )}
        {confirmNode}
      </div>
    );
  }

  return (
    <div className="cleanup">
      <div className="page-head">
        <div>
          <h1 className="page-title">Cleanup</h1>
          <p className="page-sub">Find screenshots worth reviewing and reduce library clutter.</p>
        </div>
        <div className="page-head-actions">
          <Button size="sm" variant="ghost" onClick={() => void refreshOverview()} disabled={loading}>
            {loading ? "Analyzing…" : "Refresh analysis"}
          </Button>
        </div>
      </div>
      {error && <p className="error">{error}</p>}
      {loading && !overview ? (
        <p className="muted">Analyzing screenshots…</p>
      ) : overview ? (
        <>
          <div className="cleanup-stats" role="status">
            <div className="cleanup-stat">
              <span className="cleanup-stat-value">{overview.total_count.toLocaleString()}</span>
              <span className="cleanup-stat-label">screenshots · {formatBytes(overview.total_bytes)}</span>
            </div>
            <div className="cleanup-stat accent">
              <span className="cleanup-stat-value">{formatBytes(overview.exact_reclaim_bytes)}</span>
              <span className="cleanup-stat-label">exact duplicate savings</span>
            </div>
            <div className="cleanup-stat">
              <span className="cleanup-stat-value">
                {formatBytes(
                  overview.near_candidate_bytes + overview.burst_bytes + overview.old.bytes + overview.large.bytes
                )}
              </span>
              <span className="cleanup-stat-label">review candidates (may overlap)</span>
            </div>
          </div>
          <div className="cleanup-rows">
            <section className="cleanup-row">
              <span className="set-ic"><Icons.copy size={18} /></span>
              <div className="cleanup-row-main">
                <h3>Exact duplicates</h3>
                <p className="muted small">
                  {overview.exact_count.toLocaleString()} screenshots in {overview.exact_groups.toLocaleString()} groups ·
                  keep one copy per group to recover {formatBytes(overview.exact_reclaim_bytes)}
                </p>
              </div>
              <Button size="sm" onClick={() => onNavigate("duplicates")} disabled={overview.exact_groups === 0}>
                Review
              </Button>
            </section>
            <section className="cleanup-row">
              <span className="set-ic"><Icons.image size={18} /></span>
              <div className="cleanup-row-main">
                <h3>Near duplicates</h3>
                <p className="muted small">
                  {overview.near_count.toLocaleString()} screenshots in {overview.near_groups.toLocaleString()} similar groups ·
                  up to {formatBytes(overview.near_candidate_bytes)} if you reviewed them all
                </p>
              </div>
              <Button size="sm" onClick={() => onNavigate("duplicates")} disabled={overview.near_groups === 0}>
                Review
              </Button>
            </section>
            <section className="cleanup-row">
              <span className="set-ic"><Icons.zap size={18} /></span>
              <div className="cleanup-row-main">
                <h3>Bursts</h3>
                <p className="muted small">
                  {overview.burst_groups.toLocaleString()} session{overview.burst_groups === 1 ? "" : "s"} ·{" "}
                  {overview.burst_count.toLocaleString()} screenshots ·{" "}
                  {formatBytes(overview.burst_bytes)}
                </p>
              </div>
              <Button size="sm" onClick={() => onNavigate("bursts")} disabled={overview.burst_groups === 0}>
                Review
              </Button>
            </section>
            <section className="cleanup-row">
              <span className="set-ic"><Icons.clock size={18} /></span>
              <div className="cleanup-row-main">
                <h3>Old screenshots</h3>
                <p className="muted small">
                  {overview.old.count.toLocaleString()} older than{" "}
                  {AGE_OPTIONS.find((o) => o.days === overview.old_age_days)?.label ?? "1 year"} ·{" "}
                  {formatBytes(overview.old.bytes)}
                </p>
              </div>
              <Button size="sm" onClick={() => { setAgeDays(overview.old_age_days); openReview("old"); }} disabled={overview.old.count === 0}>
                Review
              </Button>
            </section>
            <section className="cleanup-row">
              <span className="set-ic"><Icons.drive size={18} /></span>
              <div className="cleanup-row-main">
                <h3>Large screenshots</h3>
                <p className="muted small">
                  {overview.large.count.toLocaleString()} over{" "}
                  {SIZE_OPTIONS.find((o) => o.bytes === overview.large_min_bytes)?.label.replace("> ", "") ?? ""} ·{" "}
                  {formatBytes(overview.large.bytes)}
                </p>
              </div>
              <Button size="sm" onClick={() => { setMinBytes(overview.large_min_bytes); openReview("large"); }} disabled={overview.large.count === 0}>
                Review
              </Button>
            </section>
            <section className="cleanup-row">
              <span className="set-ic"><Icons.fileText size={18} /></span>
              <div className="cleanup-row-main">
                <h3>No detected text</h3>
                <p className="muted small">
                  {overview.notext.count.toLocaleString()} screenshots · {formatBytes(overview.notext.bytes)} ·
                  review filter only, not a deletion recommendation
                </p>
              </div>
              <Button size="sm" onClick={() => openReview("notext")} disabled={overview.notext.count === 0}>
                Review
              </Button>
            </section>
          </div>
          {(memEnabled || memTotal > 0) && (
            <section className="cleanup-memories">
              <div className="mem-head">
                <div>
                  <h3>Deleted memories</h3>
                  <p className="muted small">
                    Searchable records of trashed screenshots — metadata only, not backups.{" "}
                    {memTotal.toLocaleString()} retained.
                  </p>
                </div>
                {memTotal > 0 && (
                  <Button size="sm" variant="ghost" onClick={() => void clearMemories()}>
                    Clear all
                  </Button>
                )}
              </div>
              <input
                className="mem-filter"
                type="search"
                placeholder="Filter by filename or text…"
                aria-label="Filter deleted memories"
                value={memQuery}
                onChange={(e) => setMemQuery(e.target.value)}
              />
              {memories.length === 0 ? (
                <p className="muted small">{memQuery ? "No records match." : "No records retained yet."}</p>
              ) : (
                <ul className="mem-list">
                  {memories.map((m) => (
                    <li key={m.id} className="mem-row">
                      <button className="mem-main" onClick={() => setMemViewing(m)} title="View record">
                        <span className="shot-name">{m.filename}</span>
                        <span className="shot-date">
                          Deleted {utcToLocal(m.deleted_at)} · {formatBytes(m.size)}
                          {m.ocr_text.trim() ? ` · ${m.ocr_text.trim().slice(0, 80)}` : ""}
                        </span>
                      </button>
                      <button
                        className="iconbtn"
                        title="Permanently delete this record"
                        aria-label={`Permanently delete record ${m.filename}`}
                        onClick={() => void deleteMemory(m)}
                      >
                        ✕
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          )}
          {analyzedLabel && (
            <p className="muted small" style={{ marginTop: 12 }}>
              Last analyzed {analyzedLabel} · from indexed metadata, no files touched.
            </p>
          )}
        </>
      ) : null}
      {memViewing && <MemoryViewer memory={memViewing} onClose={() => setMemViewing(null)} />}
      {confirmNode}
    </div>
  );
}
