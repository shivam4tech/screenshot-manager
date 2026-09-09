import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  thumbnailUrl,
  type Burst,
  type CollectionInfo,
  type ScreenshotRow,
} from "../api";
import { BulkBar, useSelection } from "./bulk";
import { useInfiniteLoader } from "./scroll";
import { Icons } from "./icons";
import { Button, Dropdown, ToastStack, useToasts } from "./ui";
import { ScreenshotCard } from "./ScreenshotCard";
import Cull from "./Cull";

const PAGE_SIZE = 60;
const GAP_OPTIONS = [
  { secs: 900, label: "15 min gaps" },
  { secs: 1800, label: "30 min gaps" },
  { secs: 7200, label: "2 hour gaps" },
];

function burstTitle(b: Burst): string {
  const day = new Date(b.start_ts * 1000).toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  });
  const fmt = (ts: number) =>
    new Date(ts * 1000).toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
    });
  return `${day}, ${fmt(b.start_ts)} – ${fmt(b.end_ts)}`;
}

/**
 * Bursts: capture-time clusters ("that afternoon I researched X") with
 * theme hints, preview strips, drill-down grid, and bulk organization.
 */
export default function Bursts({
  onOpenDetail,
  collections,
  refreshOrganize,
}: {
  onOpenDetail: (id: number) => void;
  collections: CollectionInfo[];
  refreshOrganize: () => void;
}) {
  const [bursts, setBursts] = useState<Burst[]>([]);
  const [gap, setGap] = useState(1800);
  const [openKey, setOpenKey] = useState<string | null>(null);
  const [items, setItems] = useState<ScreenshotRow[]>([]);
  const [thumbs, setThumbs] = useState<Map<number, string>>(new Map());
  const [previews, setPreviews] = useState<Map<string, string>>(new Map());
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [culling, setCulling] = useState(false);
  const [selectingAll, setSelectingAll] = useState(false);
  const sel = useSelection();
  const trashHotkeyRef = useRef<(() => void) | null>(null);
  const { toasts, push: toast, dismiss: dismissToast, pause: pauseToast, resume: resumeToast } = useToasts();

  /* Delete key trashes the current selection (same confirm flow as the button). */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const typing = !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
      if ((e.key === "Delete" || e.key === "Backspace") && !typing && sel.selected.size > 0) {
        e.preventDefault();
        trashHotkeyRef.current?.();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sel.selected.size]);

  const resolveThumbs = useCallback(
    async (rows: { id: number; content_hash: string | null }[]) => {
      const fresh = new Map<number, string>();
      for (const row of rows) {
        const url = await thumbnailUrl(row.content_hash, 512);
        if (url) fresh.set(row.id, url);
      }
      setThumbs((m) => new Map([...m, ...fresh]));
    },
    []
  );

  const reload = useCallback(async () => {
    setError(null);
    try {
      setBursts(await api.listBursts(gap));
    } catch (e) {
      setError(String(e));
    }
  }, [gap]);

  useEffect(() => {
    reload();
  }, [reload]);

  // Preview strips for collapsed burst cards.
  useEffect(() => {
    let alive = true;
    (async () => {
      const fresh = new Map<string, string>();
      for (const b of bursts) {
        for (const h of b.preview_hashes) {
          if (!h || fresh.has(h)) continue;
          const url = await thumbnailUrl(h, 256);
          if (url) fresh.set(h, url);
        }
      }
      if (alive) setPreviews(fresh);
    })();
    return () => {
      alive = false;
    };
  }, [bursts]);

  const openBurst = (b: Burst) => {
    if (openKey === b.key) {
      setOpenKey(null);
      setItems([]);
      sel.clear();
      return;
    }
    setOpenKey(b.key);
    setItems([]);
    sel.clear();
    setError(null);
    setLoading(true);
    api
      .burstItems(b.start_ts, b.end_ts, PAGE_SIZE, 0)
      .then((page) => {
        setHasMore(page.length === PAGE_SIZE);
        setItems(page);
        void resolveThumbs(page);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  };

  const loadMore = useCallback(() => {
    const b = bursts.find((x) => x.key === openKey);
    if (!b || loading) return;
    setLoading(true);
    api
      .burstItems(b.start_ts, b.end_ts, PAGE_SIZE, items.length)
      .then((page) => {
        setHasMore(page.length === PAGE_SIZE);
        setItems((r) => [...r, ...page]);
        void resolveThumbs(page);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [bursts, openKey, items.length, loading, resolveThumbs]);

  const sentinel = useInfiniteLoader(openKey !== null && hasMore, loading, loadMore);

  const undoTrash = async (ids: number[]) => {
    try {
      const s = await api.restoreScreenshots(ids);
      const ok = ids.filter((id) => !s.failed.some((f) => f.id === id));
      reload();
      refreshOrganize();
      if (ok.length > 0) sel.selectAll(ok);
      toast(
        ok.length === ids.length
          ? `Restored ${ok.length} screenshot${ok.length === 1 ? "" : "s"}.`
          : `Restored ${ok.length} of ${ids.length}.`
      );
    } catch (e) {
      setError(String(e));
    }
  };

  const afterBulk = (removedIds: number[]) => {
    if (removedIds.length > 0) {
      setItems((rows) => rows.filter((r) => !removedIds.includes(r.id)));
      sel.remove(removedIds);
      toast(`${removedIds.length} screenshot${removedIds.length === 1 ? "" : "s"} moved to trash`, {
        label: "Undo",
        fn: () => void undoTrash(removedIds),
      });
    } else {
      sel.clear();
    }
    refreshOrganize();
    reload();
  };

  const afterCull = (trashedIds: number[]) => {
    setCulling(false);
    afterBulk(trashedIds);
  };

  const openB = bursts.find((x) => x.key === openKey) ?? null;

  const selectAllInBurst = async () => {
    if (!openB) return;
    setSelectingAll(true);
    try {
      sel.selectAll(await api.burstRangeIds(openB.start_ts, openB.end_ts));
    } catch (e) {
      setError(String(e));
    } finally {
      setSelectingAll(false);
    }
  };

  return (
    <div className="bursts">
      <div className="page-head">
        <div>
          <h1 className="page-title">Bursts</h1>
          <p className="page-sub">Grouped screenshots taken in quick succession</p>
        </div>
      </div>
      <div className="dup-toolbar">
        <span className="muted small">
          {bursts.length} session{bursts.length === 1 ? "" : "s"}
        </span>
        <span className="toolbar-group">
          <Dropdown
            ariaLabel="Burst gap"
            prefix="Split after:"
            value={String(gap)}
            onChange={(v) => setGap(Number(v))}
            options={GAP_OPTIONS.map((g) => ({ value: String(g.secs), label: g.label }))}
          />
        </span>
      </div>
      {error && <p className="error">{error}</p>}
      {bursts.length === 0 && !error ? (
        <div className="empty-state">
          <h2>No bursts yet.</h2>
          <p>Screenshots captured close together will cluster here automatically.</p>
        </div>
      ) : (
        bursts.map((b) => (
          <div key={b.key}>
            <button
              className={`burst-card${openKey === b.key ? " active" : ""}`}
              onClick={() => openBurst(b)}
              aria-expanded={openKey === b.key}
            >
              <span className="burst-previews" aria-hidden="true">
                {b.preview_hashes.slice(0, 4).map((h, i) =>
                  h && previews.has(h) ? (
                    <img key={i} src={previews.get(h)} alt="" loading="lazy" />
                  ) : (
                    <span key={i} className="burst-preview-empty" />
                  )
                )}
              </span>
              <span className="burst-info">
                <span className="burst-title">{burstTitle(b)}</span>
                <span className="burst-meta">
                  {b.top_app && <span className="burst-tag">{b.top_app}</span>}
                  {b.top_category && (
                    <span className="burst-tag">{b.top_category}</span>
                  )}
                  {b.top_tags.map((t) => (
                    <span className="burst-tag" key={t}>
                      #{t}
                    </span>
                  ))}
                </span>
              </span>
              <span className="burst-count">{b.count} shots</span>
              <span className="burst-arrow" aria-hidden="true"><Icons.chevronR size={16} /></span>
            </button>
            {openKey === b.key && (
              <div className="burst-open">
                <div className="burst-open-bar">
                  <button
                    className="link-btn"
                    onClick={() => setCulling(true)}
                    title="Keyboard triage this burst: → keep, x trash, u undo"
                  >
                    Cull burst
                  </button>
                </div>
                <BulkBar
                  ids={[...sel.selected]}
                  collections={collections}
                  onDone={afterBulk}
                  onError={setError}
                  selectAllLabel="Select all"
                  selectingAll={selectingAll}
                  onSelectAll={() => void selectAllInBurst()}
                  onCancel={() => sel.clear()}
                  trashHotkeyRef={trashHotkeyRef}
                />
                <div className="shot-grid">
                  {items.map((r) => (
                    <ScreenshotCard
                      key={r.id}
                      row={r}
                      thumbUrl={thumbs.get(r.id)}
                      selected={sel.selected.has(r.id)}
                      onOpen={onOpenDetail}
                      onToggleSelect={(id) => sel.toggle(id)}
                    />
                  ))}
                </div>
                <div ref={sentinel} className="scroll-sentinel" aria-hidden="true">
                  {loading ? "Loading…" : hasMore ? "" : items.length > 0 ? "End." : ""}
                </div>
                {hasMore && (
                  <div className="load-more">
                    <Button onClick={loadMore} disabled={loading}>
                      {loading ? "Loading…" : `Load more (${items.length} shown)`}
                    </Button>
                  </div>
                )}
                {culling && <Cull items={items} onDone={afterCull} />}
              </div>
            )}
          </div>
        ))
      )}
      <ToastStack toasts={toasts} onClose={dismissToast} onPause={pauseToast} onResume={resumeToast} />
    </div>
  );
}
