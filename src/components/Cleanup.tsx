import { useCallback, useEffect, useState } from "react";
import {
  api,
  thumbnailUrl,
  type CleanupItem,
  type CleanupOverview,
} from "../api";
import { ScreenshotCard } from "./ScreenshotCard";
import { useInfiniteLoader } from "./scroll";
import { Button, Dropdown, EmptyState } from "./ui";
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

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[u]}`;
}

type ReviewCategory = "old" | "large" | "notext";

/**
 * Cleanup dashboard (Sprint 1: analysis only). Overview numbers come from
 * one indexed-metadata aggregate; review grids page through candidates.
 * No deletion or rename happens here — bulk actions arrive in Sprint 2.
 */
export default function Cleanup({
  onOpenDetail,
  onNavigate,
}: {
  onOpenDetail: (id: number) => void;
  onNavigate: (view: "duplicates" | "bursts") => void;
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

  useEffect(() => {
    refreshOverview();
  }, [refreshOverview]);

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
            <button className="link-btn" onClick={() => setReview(null)}>
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
                  selected={false}
                  selectable={false}
                  onOpen={onOpenDetail}
                  onToggleSelect={() => {}}
                />
              ))}
            </div>
            <div ref={sentinel} className="scroll-sentinel" aria-hidden="true">
              {itemsLoading ? "Loading…" : !itemsHasMore && items.length > 0 ? "End." : ""}
            </div>
            <p className="muted small" style={{ textAlign: "center" }}>
              Review for now — bulk cleanup actions arrive in Sprint 2.
            </p>
          </>
        )}
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
          {analyzedLabel && (
            <p className="muted small" style={{ marginTop: 12 }}>
              Last analyzed {analyzedLabel} · from indexed metadata, no files touched.
            </p>
          )}
        </>
      ) : null}
    </div>
  );
}
