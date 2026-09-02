import { useCallback, useEffect, useRef, useState } from "react";
import { api, thumbnailUrl, type AppStateDto, type ScreenshotRow } from "../api";

const PAGE_SIZE = 60;

/**
 * Library grid (Sprint 1): responsive thumbnail grid loaded from the disk
 * thumbnail cache, paged newest-first. Search UI arrives in Sprint 2.
 */
export default function Library({
  appState,
}: {
  appState: AppStateDto | null;
}) {
  const [rows, setRows] = useState<ScreenshotRow[]>([]);
  const [thumbs, setThumbs] = useState<Map<number, string>>(new Map());
  const [hasMore, setHasMore] = useState(true);
  const [loading, setLoading] = useState(false);
  const busy = useRef(false);

  const loadPage = useCallback(async (offset: number) => {
    if (busy.current) return;
    busy.current = true;
    setLoading(true);
    try {
      const page = await api.listScreenshots(PAGE_SIZE, offset);
      setHasMore(page.length === PAGE_SIZE);
      setRows((r) => (offset === 0 ? page : [...r, ...page]));

      // Resolve thumbnail asset URLs (from disk cache) for new rows.
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

  const dateLabel = (ts: number | null) => {
    if (!ts) return "";
    return new Date(ts * 1000).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  };

  const empty = rows.length === 0 && !loading;

  return (
    <div className="library">
      <header className="library-header">
        <input
          className="search-input"
          type="search"
          placeholder="Search your screenshots… (full-text search arrives in Sprint 2)"
          disabled
          aria-label="Search screenshots"
        />
      </header>

      {empty ? (
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
            {rows.map((r) => (
              <figure key={r.id} className="cell" title={r.filename}>
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
                <figcaption>{dateLabel(r.created_ts)}</figcaption>
              </figure>
            ))}
          </div>
          {hasMore && (
            <div className="load-more">
              <button onClick={() => loadPage(rows.length)} disabled={loading}>
                {loading ? "Loading…" : "Load more"}
              </button>
            </div>
          )}
        </>
      )}

      {appState?.indexing && (
        <p className="indexing-note" role="status">
          Indexing new screenshots… new shots appear automatically.
        </p>
      )}
    </div>
  );
}
