import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  thumbnailUrl,
  type AppStateDto,
  type CollectionInfo,
  type ScreenshotRow,
  type SearchRow,
  type TagInfo,
} from "../api";
import Detail from "./Detail";
import Timeline from "./Timeline";
import Duplicates from "./Duplicates";
import Settings from "./Settings";
import Bursts from "./Bursts";
import Cull from "./Cull";
import { BulkBar, useSelection } from "./bulk";
import { useInfiniteLoader } from "./scroll";
import type { Theme } from "../theme";

const PAGE_SIZE = 60;
const SEARCH_PAGE_SIZE = 60;
const SEARCH_DEBOUNCE_MS = 250;

type View =
  | { kind: "all" }
  | { kind: "collection"; id: number; name: string }
  | { kind: "timeline" }
  | { kind: "duplicates" }
  | { kind: "bursts" }
  | { kind: "settings" };

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
  'Search text, filenames, tags… e.g. "docker error", after:2026-08-01, tag:research, is:starred, is:duplicate';

type GridRow = Pick<
  ScreenshotRow,
  "id" | "filename" | "created_ts" | "status" | "content_hash" | "starred"
> & { snippet?: string | null };

/**
 * Library: sidebar (views, tags, collections) + grid with live full-text
 * search, multi-select bulk actions, and infinite scroll. Clicking a shot
 * opens the detail overlay, where starring, tagging, notes, collection
 * membership, and deletion are edited.
 */
export default function Library({
  appState,
  theme,
  onToggleTheme,
}: {
  appState: AppStateDto | null;
  theme: Theme;
  onToggleTheme: () => void;
}) {
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
  const [culling, setCulling] = useState(false);

  // Organize state (Sprint 3)
  const [view, setView] = useState<View>({ kind: "all" });
  const [tags, setTags] = useState<TagInfo[]>([]);
  const [collections, setCollections] = useState<CollectionInfo[]>([]);
  const [newCollection, setNewCollection] = useState("");
  const [organizeError, setOrganizeError] = useState<string | null>(null);
  const [collectionItems, setCollectionItems] = useState<ScreenshotRow[]>([]);
  const [collectionHasMore, setCollectionHasMore] = useState(false);
  const [renamingId, setRenamingId] = useState<number | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const sel = useSelection();

  const resolveThumbs = useCallback(async (items: { id: number; content_hash: string | null }[]) => {
    const fresh = new Map<number, string>();
    for (const row of items) {
      const url = await thumbnailUrl(row.content_hash, 512);
      if (url) fresh.set(row.id, url);
    }
    setThumbs((m) => new Map([...m, ...fresh]));
  }, []);

  const loadPage = useCallback(
    async (offset: number) => {
      if (busy.current) return;
      busy.current = true;
      setLoading(true);
      try {
        const page = await api.listScreenshots(PAGE_SIZE, offset);
        setHasMore(page.length === PAGE_SIZE);
        setRows((r) => (offset === 0 ? page : [...r, ...page]));
        void resolveThumbs(page);
      } finally {
        busy.current = false;
        setLoading(false);
      }
    },
    [resolveThumbs]
  );

  const refreshOrganize = useCallback(async () => {
    try {
      const [t, c] = await Promise.all([api.listTags(), api.listCollections()]);
      setTags(t);
      setCollections(c);
      setOrganizeError(null);
    } catch (e) {
      setOrganizeError(String(e));
    }
  }, []);

  const loadCollectionItems = useCallback(
    async (collectionId: number, offset: number) => {
      const page = await api.listCollectionItems(collectionId, PAGE_SIZE, offset);
      setCollectionHasMore(page.length === PAGE_SIZE);
      setCollectionItems((r) => (offset === 0 ? page : [...r, ...page]));
      void resolveThumbs(page);
    },
    [resolveThumbs]
  );

  useEffect(() => {
    loadPage(0);
    refreshOrganize();
  }, [loadPage, refreshOrganize]);

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
        void resolveThumbs(out.rows);
      })
      .catch((e) => alive && setSearchError(String(e)))
      .finally(() => alive && setSearching(false));
    return () => {
      alive = false;
    };
  }, [activeQuery, resolveThumbs]);

  const inSearch = activeQuery.length > 0;
  const inCollection = !inSearch && view.kind === "collection";
  const inSpecial =
    !inSearch &&
    (view.kind === "timeline" ||
      view.kind === "duplicates" ||
      view.kind === "bursts" ||
      view.kind === "settings");

  // Selection never survives a context switch.
  useEffect(() => {
    sel.clear();
  }, [activeQuery, view]); // eslint-disable-line react-hooks/exhaustive-deps

  const selectCollection = (c: CollectionInfo) => {
    setQuery("");
    setView({ kind: "collection", id: c.id, name: c.name });
    setCollectionItems([]);
    loadCollectionItems(c.id, 0).catch((e) => setOrganizeError(String(e)));
  };

  const refreshAfterChange = useCallback(() => {
    loadPage(0);
    refreshOrganize();
    if (view.kind === "collection") {
      loadCollectionItems(view.id, 0).catch(() => {});
    }
  }, [loadPage, refreshOrganize, loadCollectionItems, view]);

  /** After a bulk action: drop trashed rows, clear selection, refresh counts. */
  const afterBulk = useCallback(
    (removedIds: number[]) => {
      if (removedIds.length > 0) {
        const gone = new Set(removedIds);
        setRows((r) => r.filter((row) => !gone.has(row.id)));
        setCollectionItems((r) => r.filter((row) => !gone.has(row.id)));
        setSearchOutcome((o) =>
          o ? { total: o.total - removedIds.length, rows: o.rows.filter((row) => !gone.has(row.id)) } : o
        );
      }
      sel.clear();
      refreshOrganize();
    },
    [refreshOrganize] // eslint-disable-line react-hooks/exhaustive-deps
  );

  /** Next page for the visible grid (ranked search stays top-N by design). */
  const loadMoreNext = useCallback(() => {
    if (inSearch) return;
    if (inCollection && view.kind === "collection") {
      loadCollectionItems(view.id, collectionItems.length).catch((e) =>
        setOrganizeError(String(e))
      );
    } else {
      loadPage(rows.length);
    }
  }, [inSearch, inCollection, view, collectionItems.length, rows.length, loadCollectionItems, loadPage]);
  const gridHasMore = inSearch ? false : inCollection ? collectionHasMore : hasMore;
  const sentinel = useInfiniteLoader(gridHasMore, loading, loadMoreNext);

  const createCollection = () => {
    const name = newCollection.trim();
    if (!name) return;
    setNewCollection("");
    api
      .createCollection(name)
      .then((c) => {
        refreshOrganize();
        selectCollection(c);
      })
      .catch((e) => setOrganizeError(String(e)));
  };

  const deleteCollection = (c: CollectionInfo) => {
    if (!window.confirm(`Delete collection “${c.name}”? Screenshots are kept.`)) return;
    api
      .deleteCollection(c.id)
      .then(() => {
        if (view.kind === "collection" && view.id === c.id) setView({ kind: "all" });
        refreshOrganize();
      })
      .catch((e) => setOrganizeError(String(e)));
  };

  const saveRename = (c: CollectionInfo) => {
    const name = renameDraft.trim();
    setRenamingId(null);
    if (!name || name === c.name) return;
    api
      .renameCollection(c.id, name)
      .then(() => {
        refreshOrganize();
        if (view.kind === "collection" && view.id === c.id) {
          setView({ kind: "collection", id: c.id, name });
        }
      })
      .catch((e) => setOrganizeError(String(e)));
  };

  const gridRows: GridRow[] = inSearch
    ? (searchOutcome?.rows ?? [])
    : inCollection
      ? collectionItems
      : rows;

  const dateLabel = (ts: number | null) => {
    if (!ts) return "";
    return new Date(ts * 1000).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  };

  const emptyLibrary = !inSearch && !inCollection && rows.length === 0 && !loading;
  const noResults =
    inSearch && !searching && !searchError && (searchOutcome?.rows.length ?? 0) === 0;
  const emptyCollection =
    inCollection && collectionItems.length === 0;

  return (
    <div className="library-shell">
      <aside className="sidebar" aria-label="Organize">
        <nav className="side-section">
          <button
            className={`side-item${view.kind === "all" && !inSearch ? " active" : ""}`}
            onClick={() => {
              setQuery("");
              setView({ kind: "all" });
            }}
          >
            All screenshots
          </button>
          <button
            className={`side-item${activeQuery === "is:starred" ? " active" : ""}`}
            onClick={() => {
              setView({ kind: "all" });
              setQuery("is:starred");
            }}
            title="Search is:starred"
          >
            ★ Starred
          </button>
          <button
            className={`side-item${view.kind === "timeline" && !inSearch ? " active" : ""}`}
            onClick={() => {
              setQuery("");
              setView({ kind: "timeline" });
            }}
            title="Browse by capture date"
          >
            ◷ Timeline
          </button>
          <button
            className={`side-item${view.kind === "duplicates" && !inSearch ? " active" : ""}`}
            onClick={() => {
              setQuery("");
              setView({ kind: "duplicates" });
            }}
            title="Review exact and similar duplicates"
          >
            ⧉ Duplicates
          </button>
          <button
            className={`side-item${view.kind === "bursts" && !inSearch ? " active" : ""}`}
            onClick={() => {
              setQuery("");
              setView({ kind: "bursts" });
            }}
            title="Capture-time clusters with theme hints"
          >
            ◍ Bursts
          </button>
          <button
            className={`side-item${view.kind === "settings" && !inSearch ? " active" : ""}`}
            onClick={() => {
              setQuery("");
              setView({ kind: "settings" });
            }}
            title="Folders, OCR, enrichment, index health"
          >
            ⚙ Settings
          </button>
        </nav>

        <div className="side-section">
          <h4>Tags</h4>
          {tags.length === 0 ? (
            <p className="muted small">No tags yet — add one in the detail view.</p>
          ) : (
            <ul className="side-list">
              {tags.map((t) => (
                <li key={t.name}>
                  <button
                    className="side-item"
                    onClick={() => {
                      setView({ kind: "all" });
                      setQuery(`tag:"${t.name}"`);
                    }}
                    title={`Search tag:${t.name}`}
                  >
                    <span className="side-label">{t.name}</span>
                    <span className="side-count">{t.count}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="side-section">
          <h4>Collections</h4>
          {collections.length > 0 && (
            <ul className="side-list">
              {collections.map((c) =>
                renamingId === c.id ? (
                  <li key={c.id}>
                    <input
                      className="rename-input"
                      autoFocus
                      value={renameDraft}
                      aria-label={`Rename ${c.name}`}
                      onChange={(e) => setRenameDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") saveRename(c);
                        if (e.key === "Escape") setRenamingId(null);
                      }}
                      onBlur={() => saveRename(c)}
                    />
                  </li>
                ) : (
                  <li key={c.id} className="side-row">
                    <button
                      className={`side-item${
                        view.kind === "collection" && view.id === c.id && !inSearch
                          ? " active"
                          : ""
                      }`}
                      onClick={() => selectCollection(c)}
                      title={`Open collection ${c.name}`}
                    >
                      <span className="side-label">{c.name}</span>
                      <span className="side-count">{c.item_count}</span>
                    </button>
                    <span className="side-ops">
                      <button
                        className="icon-btn"
                        title={`Rename ${c.name}`}
                        aria-label={`Rename ${c.name}`}
                        onClick={() => {
                          setRenamingId(c.id);
                          setRenameDraft(c.name);
                        }}
                      >
                        ✎
                      </button>
                      <button
                        className="icon-btn"
                        title={`Delete ${c.name}`}
                        aria-label={`Delete ${c.name}`}
                        onClick={() => deleteCollection(c)}
                      >
                        ✕
                      </button>
                    </span>
                  </li>
                )
              )}
            </ul>
          )}
          <div className="collection-add">
            <input
              type="text"
              placeholder="New collection…"
              aria-label="New collection name"
              value={newCollection}
              onChange={(e) => setNewCollection(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") createCollection();
              }}
            />
            <button onClick={createCollection} disabled={!newCollection.trim()}>
              Add
            </button>
          </div>
        </div>

        {organizeError && <p className="error small">{organizeError}</p>}

        <div className="side-footer">
          <button
            className="theme-toggle"
            onClick={onToggleTheme}
            title={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
            aria-label="Toggle color theme"
          >
            {theme === "dark" ? "☀ Light" : "☾ Dark"}
          </button>
        </div>
      </aside>

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
          {inSearch ? (
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
          ) : (
            inCollection &&
            view.kind === "collection" && (
              <div className="search-meta" role="status">
                <span className="muted">
                  Collection “{view.name}” — {collectionItems.length} shown
                </span>
                <button className="link-btn" onClick={() => setView({ kind: "all" })}>
                  Show all
                </button>
              </div>
            )
          )}
        </header>

        {!inSpecial && !searchError && gridRows.length > 0 && (
          <div className="view-toolbar">
            <button
              className="link-btn"
              onClick={() => setCulling(true)}
              title="Keyboard triage: → keep, x trash, u undo"
            >
              ⌨ Cull {gridRows.length} shots
            </button>
          </div>
        )}

        {inSpecial ? (
          view.kind === "timeline" ? (
            <Timeline onOpenDetail={(id) => setDetailId(id)} />
          ) : view.kind === "duplicates" ? (
            <Duplicates onOpenDetail={(id) => setDetailId(id)} onChanged={refreshOrganize} />
          ) : view.kind === "bursts" ? (
            <Bursts
              onOpenDetail={(id) => setDetailId(id)}
              collections={collections}
              refreshOrganize={refreshOrganize}
            />
          ) : (
            <Settings
              onFoldersChanged={() => {
                loadPage(0);
                refreshOrganize();
              }}
            />
          )
        ) : searchError ? (
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
        ) : emptyCollection ? (
          <div className="empty-state">
            <h2>Empty collection.</h2>
            <p>Open a screenshot and add it via “In collections”.</p>
          </div>
        ) : (
          <>
            {gridRows.length > 0 && (
              <div className="select-all-row">
                <label className="muted small">
                  <input
                    type="checkbox"
                    checked={gridRows.every((r) => sel.selected.has(r.id))}
                    onChange={(e) =>
                      e.target.checked
                        ? sel.selectAll(gridRows.map((r) => r.id))
                        : sel.clear()
                    }
                  />{" "}
                  Select all shown ({gridRows.length})
                </label>
              </div>
            )}
            <div className="grid">
              {gridRows.map((r) => (
                <figure
                  key={r.id}
                  className={`cell clickable${sel.selected.has(r.id) ? " selected" : ""}`}
                  title={r.filename}
                  onClick={() => setDetailId(r.id)}
                >
                  <div className="thumb-box">
                    {thumbs.has(r.id) ? (
                      <img src={thumbs.get(r.id)} alt={r.filename} loading="lazy" />
                    ) : (
                      <div className="thumb-placeholder" aria-hidden="true" />
                    )}
                    <span
                      className="cell-select"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <input
                        type="checkbox"
                        checked={sel.selected.has(r.id)}
                        onChange={() => sel.toggle(r.id)}
                        aria-label={`Select ${r.filename}`}
                      />
                    </span>
                    {r.status !== "available" && (
                      <span className={`badge badge-${r.status}`}>{r.status}</span>
                    )}
                    {r.starred && <span className="star-badge" title="Starred">★</span>}
                    {inCollection && view.kind === "collection" && (
                      <button
                        className="cell-remove"
                        title="Remove from collection"
                        aria-label={`Remove ${r.filename} from collection`}
                        onClick={(e) => {
                          e.stopPropagation();
                          api
                            .removeFromCollection(view.id, r.id)
                            .then(() => {
                              setCollectionItems((items) =>
                                items.filter((it) => it.id !== r.id)
                              );
                              refreshOrganize();
                            })
                            .catch((err) => setOrganizeError(String(err)));
                        }}
                      >
                        ✕
                      </button>
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
            {sel.selected.size > 0 && (
              <BulkBar
                ids={[...sel.selected]}
                collections={collections}
                onDone={afterBulk}
                onError={setOrganizeError}
              />
            )}
            {inSearch && searchOutcome && gridRows.length < searchOutcome.total && (
              <p className="muted small" style={{ textAlign: "center" }}>
                Showing the top {gridRows.length} of {searchOutcome.total} — refine
                your query to narrow it down.
              </p>
            )}
            <div ref={sentinel} className="scroll-sentinel" aria-hidden="true">
              {!inSearch && loading ? "Loading…" : !gridHasMore && gridRows.length > 0 ? "End." : ""}
            </div>
            {!inSearch && gridHasMore && (
              <div className="load-more">
                <button onClick={loadMoreNext} disabled={loading}>
                  {loading ? "Loading…" : `Load more (${gridRows.length} shown)`}
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

        {detailId !== null && (
          <Detail
            id={detailId}
            onClose={() => setDetailId(null)}
            onChanged={refreshAfterChange}
          />
        )}

        {culling && (
          <Cull
            items={gridRows}
            onDone={(trashedIds) => {
              setCulling(false);
              afterBulk(trashedIds);
            }}
          />
        )}
      </div>
    </div>
  );
}
