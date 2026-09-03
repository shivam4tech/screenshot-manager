import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  thumbnailUrl,
  type AppStateDto,
  type ScreenshotRow,
  type SearchRow,
} from "../api";
import Detail from "./Detail";

const PAGE_SIZE = 60;
const SEARCH_PAGE_SIZE = 60;
const SEARCH_DEBOUNCE_MS = 250;

/** Render a snippet with [match] marks as bold nodes. */
function Snippet({ text }: { text: string }) {
  const parts = text.split(/(\[[^\]]*\])/g);
  return (
    <>
      {parts.map((p, i) =>
        p.startsWith("[") && p.endsWith("]") ? <b key={i}>{p.slice(1, -1)}</b> : p
      )}
    </>
  );
}

const SEARCH_HINT =
  'Search text, filenames, tags… e.g. "docker error", after:2026-08-01, tag:research, is:duplicate';

type GridRow = Pick<
  ScreenshotRow,
  "id" | "filename" | "created_ts" | "status" | "content_hash"
> & { snippet?: string | null };

/**
 * Library: paged newest-first grid, with live full-text search over
 * filenames + OCR text (ranked, with highlighted snippets). Clicking a
 * shot opens the detail overlay.
 */
export default function Library({ appState }: { appState: AppStateDto | null }) {
  const [rows, setRows] = useState<ScreenshotRow[]>([]);
  const [thumbs, setThumbs] = useState<Map<number, string>>(new Map());
  const [hasMore, setHasMore] = useState(true);
  const [loading, setLoading] = useState(false);
  const busy = useRef(false);

  // Search state (Sprint 2)
  const [query, setQuery] = useState("");
  const [activeQuery, setActiveQuery] = useState(""); // debounced
  const [searchOutcome, setSearchOutcome] = useState<{
    total: number;
    rows: SearchRow[];
  } | null>(null);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [detailId, setDetailId] = useState<number | null>(null);

  const loadPage = useCallback(async (offset: number) => {
    if (busy.current) return;
    busy.current = true;
    setLoading(true);
    try {
      const page = await api.listScreenshots(PAGE_SIZE, offset);
      setHasMore(page.length === PAGE_SIZE);
      setRows((r) => (offset === 0 ? page : [...r, ...page]));

      const newThumbs = new Map<number, string>();
      for (const row of page) {
        const url = await thumbnailUrl(row.content_hash, 512);
        if (url) newThumbs.set(row.id, url);
      }
      setThumbs((m) => new Map([...m, ...newThumbs]));
    } finally {
      busy.current = false;
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadPage(0);
  }, [loadPage]);

  // Debounce the query, then run a ranked search when it's non-empty.
  useEffect(() => {
    const t = setTimeout(() => setActiveQuery(query.trim()), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [query]);

  useEffect(() => {
    if (!activeQuery) {
      setSearchOutcome(null);
      setSearchError(null);
      return;
    }
    let alive = true;
    setSearching(true);
    setSearchError(null);
    api
      .search(activeQuery, SEARCH_PAGE_SIZE, 0)
      .then((out) => {
        if (!alive) return;
        setSearchOutcome({ total: out.total, rows: out.rows });
        // Resolve thumbnails for the result page.
        (async () => {
          const newThumbs = new Map<number, string>();
          for (const row of out.rows) {
            const url = await thumbnailUrl(row.content_hash, 512);
            if (url) newThumbs.set(row.id, url);
          }
          if (alive) setThumbs((m) => new Map([...m, ...newThumbs]));
        })();
      })
      .catch((e) => alive && setSearchError(String(e)))
      .finally(() => alive && setSearching(false));
    return () => {
      alive = false;
    };
  }, [activeQuery]);

  const inSearch = activeQuery.length > 0;
  const gridRows: GridRow[] = inSearch ? (searchOutcome?.rows ?? []) : rows;

  const dateLabel = (ts: number | null) => {
    if (!ts) return "";
    return new Date(ts * 1000).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  };

  const emptyLibrary = !inSearch && rows.length === 0 && !loading;
  const noResults =
    inSearch && !searching && !searchError && (searchOutcome?.rows.length ?? 0) === 0;

  return (
    <div className="library">
      <header className="library-header">
        <input
          className="search-input"
          type="search"
          placeholder={SEARCH_HINT}
          aria-label="Search screenshots"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {inSearch && (
          <div className="search-meta" role="status">
            {searching ? (
              <span className="muted">Searching…</span>
            ) : (
              <span className="muted">
                {searchOutcome
                  ? `${searchOutcome.total.toLocaleString()} result${
                      searchOutcome.total === 1 ? "" : "s"
                    } for “${activeQuery}”`
                  : ""}
              </span>
            )}
            <button className="link-btn" onClick={() => setQuery("")}>
              Clear
            </button>
          </div>
        )}
      </header>

      {searchError ? (
        <div className="empty-state">
          <h2>Search failed</h2>
          <p className="error">{searchError}</p>
        </div>
      ) : noResults ? (
        <div className="empty-state">
          <h2>No matches.</h2>
          <p>
            Try fewer words, quoted phrases like <span className="mono">"exact text"</span>,
            or filters like <span className="mono">has:text</span>,{" "}
            <span className="mono">last month</span>,{" "}
            <span className="mono">type:png</span>.
          </p>
        </div>
      ) : emptyLibrary ? (
        <div className="empty-state">
          <h2>No screenshots yet.</h2>
          <p>
            Add a screenshot folder in Settings and we'll build your searchable
            visual memory automatically.
          </p>
        </div>
      ) : (
        <>
          <div className="grid">
            {gridRows.map((r) => (
              <figure
                key={r.id}
                className="cell clickable"
                title={r.filename}
                onClick={() => setDetailId(r.id)}
              >
                <div className="thumb-box">
                  {thumbs.has(r.id) ? (
                    <img src={thumbs.get(r.id)} alt={r.filename} loading="lazy" />
                  ) : (
                    <div className="thumb-placeholder" aria-hidden="true" />
                  )}
                  {r.status !== "available" && (
                    <span className={`badge badge-${r.status}`}>{r.status}</span>
                  )}
                </div>
                {"snippet" in r && r.snippet ? (
                  <figcaption className="snippet">
                    <Snippet text={r.snippet} />
                  </figcaption>
                ) : (
                  <figcaption>{dateLabel(r.created_ts)}</figcaption>
                )}
              </figure>
            ))}
          </div>
          {!inSearch && hasMore && (
            <div className="load-more">
              <button onClick={() => loadPage(rows.length)} disabled={loading}>
                {loading ? "Loading…" : "Load more"}
              </button>
            </div>
          )}
          {inSearch &&
            searchOutcome &&
            gridRows.length < searchOutcome.total && (
              <p className="muted small" style={{ textAlign: "center" }}>
                Showing the top {gridRows.length} of {searchOutcome.total} — refine
                your query to narrow it down.
              </p>
            )}
        </>
      )}

      {appState?.indexing && (
        <p className="indexing-note" role="status">
          Indexing new screenshots… new shots appear automatically.
        </p>
      )}

      {detailId !== null && (
        <Detail id={detailId} onClose={() => setDetailId(null)} />
      )}
    </div>
  );
}
