import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  thumbnailUrl,
  type AppStateDto,
  type CollectionInfo,
  type DirectoryDto,
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
import { Icons, type IconName } from "./icons";
import { Button, CountBadge, Dropdown, EmptyState, IconButton, SearchBar, ToastStack, Toggle, useToasts } from "./ui";
import { ScreenshotCard, displayName } from "./ScreenshotCard";
import type { Accent, Theme } from "../theme";

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

type ViewMode = "grid" | "list";

const SEARCH_HINT = "Search screenshots, text, filenames, tags…";

/* Query-token helpers: filter pills read/write tokens inside the single
   search string, so filtering stays 100% backend-driven (no new APIs). */
function tokenRegex(key: string) {
  return new RegExp(`(^|\\s)${key}:("[^"]*"|\\S+)`);
}
function getToken(q: string, key: string): string | null {
  const m = q.match(tokenRegex(key));
  return m ? m[2].replace(/^"|"$/g, "") : null;
}
function setToken(q: string, key: string, value: string | null): string {
  const clean = q.replace(tokenRegex(key), "$1").replace(/\s+/g, " ").trim();
  if (value === null) return clean;
  const tok = `${key}:${/\s/.test(value) ? `"${value}"` : value}`;
  return clean ? `${clean} ${tok}` : tok;
}
function hasFlag(q: string, flag: string) {
  return new RegExp(`(^|\\s)${flag}(?=\\s|$)`).test(q);
}
function toggleFlag(q: string, flag: string) {
  return hasFlag(q, flag)
    ? q.replace(new RegExp(`(^|\\s)${flag}(?=\\s|$)`), " ").replace(/\s+/g, " ").trim()
    : q ? `${q} ${flag}` : flag;
}
function isoDay(d: Date) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}
function daysAgo(n: number) {
  const d = new Date();
  d.setDate(d.getDate() - n);
  return isoDay(d);
}

const TYPE_OPTIONS = ["png", "jpg", "gif", "webp"];

type GridRow = Pick<
  ScreenshotRow,
  "id" | "filename" | "created_ts" | "status" | "content_hash" | "starred"
> & { snippet?: string | null };

const NAV: Array<{ kind: View["kind"]; label: string; icon: IconName; title: string }> = [
  { kind: "all", label: "All Screenshots", icon: "grid", title: "Browse everything" },
  { kind: "timeline", label: "Timeline", icon: "clock", title: "Browse by capture date" },
  { kind: "duplicates", label: "Duplicates", icon: "copy", title: "Review exact and similar duplicates" },
  { kind: "bursts", label: "Bursts", icon: "zap", title: "Capture-time clusters with theme hints" },
  { kind: "settings", label: "Settings", icon: "settings", title: "Appearance, OCR, enrichment, index health" },
];

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
  accent,
  onAccentChange,
  customHex,
  onCustomAccent,
}: {
  appState: AppStateDto | null;
  theme: Theme;
  onToggleTheme: () => void;
  accent: Accent;
  onAccentChange: (a: Accent) => void;
  customHex: string;
  onCustomAccent: (hex: string) => void;
}) {
  const [rows, setRows] = useState<ScreenshotRow[]>([]);
  const [thumbs, setThumbs] = useState<Map<number, string>>(new Map());
  const [hasMore, setHasMore] = useState(true);
  const [loading, setLoading] = useState(false);
  const busy = useRef(false);
  const searchRef = useRef<HTMLInputElement>(null);

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
  const [viewMode, setViewMode] = useState<ViewMode>("grid");

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
  const [totalAll, setTotalAll] = useState(0);
  const [starredCount, setStarredCount] = useState<number | null>(null);
  const [dirs, setDirs] = useState<DirectoryDto[]>([]);
  const [selectingAll, setSelectingAll] = useState(false);
  const [selectMode, setSelectMode] = useState(false);
  const sel = useSelection();
  const { toasts, push: toast, dismiss: dismissToast, pause: pauseToast, resume: resumeToast } = useToasts();

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
      const [t, c, s, d] = await Promise.all([
        api.listTags(),
        api.listCollections(),
        api.getStats(),
        api.listDirectories(),
      ]);
      setTags(t);
      setCollections(c);
      setTotalAll(s.total);
      setDirs(d);
      setOrganizeError(null);
      api.search("is:starred", 1, 0).then((o) => setStarredCount(o.total)).catch(() => {});
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

  // Ctrl/Cmd+K focuses search.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        searchRef.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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
    setSelectMode(false);
  }, [activeQuery, view]); // eslint-disable-line react-hooks/exhaustive-deps

  const toggleSel = useCallback((id: number) => {
    sel.toggle(id);
    setSelectMode(true);
  }, [sel]);

  const exitSelectMode = useCallback(() => {
    sel.clear();
    setSelectMode(false);
  }, [sel]);

  const addFolder = async () => {
    setOrganizeError(null);
    try {
      const picked = await api.pickFolder();
      if (!picked) return;
      await api.addDirectory(picked);
      toast(`Watching ${picked}`);
      loadPage(0);
      refreshOrganize();
    } catch (e) {
      setOrganizeError(String(e));
    }
  };

  const removeFolder = async (d: DirectoryDto) => {
    if (!window.confirm(`Stop watching “${d.path}”?\n\nIndexed records are kept.`)) return;
    setOrganizeError(null);
    try {
      await api.removeDirectory(d.id);
      if (getToken(query, "dir") === d.path || getToken(query, "folder") === d.path) {
        setQuery("");
      }
      loadPage(0);
      refreshOrganize();
    } catch (e) {
      setOrganizeError(String(e));
    }
  };

  const selectCollection = (c: CollectionInfo) => {    setQuery("");
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

  /** Optimistic star toggle from the grid (no scroll loss). */
  const toggleStar = useCallback((id: number, next: boolean) => {
    const patch = <T extends { id: number; starred: boolean }>(r: T): T =>
      r.id === id ? { ...r, starred: next } : r;
    setRows((r) => r.map(patch));
    setCollectionItems((r) => r.map(patch));
    setSearchOutcome((o) => (o ? { total: o.total, rows: o.rows.map(patch) } : o));
    api.setStarred(id, next).then(() => {
      toast(next ? "Screenshot starred" : "Screenshot unstarred");
      refreshOrganize();
    }).catch((e) => setOrganizeError(String(e)));
  }, [refreshOrganize, toast]);

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
        toast(`${removedIds.length} screenshot${removedIds.length === 1 ? "" : "s"} moved to trash`);
      }
      sel.clear();
      refreshOrganize();
    },
    [refreshOrganize, toast] // eslint-disable-line react-hooks/exhaustive-deps
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
        toast(`Collection “${c.name}” created`);
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
        toast(`Collection “${c.name}” deleted`);
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

  // Total behind the current view (what "select all" means — the grid only
  // renders a window of it via infinite scroll).
  const viewTotal = inSearch
    ? (searchOutcome?.total ?? 0)
    : inCollection && view.kind === "collection"
      ? (collections.find((c) => c.id === view.id)?.item_count ?? collectionItems.length)
      : totalAll;
  const watchedCount = dirs.filter((d) => d.enabled).length;
  const pageSub: string | null = inSearch
    ? searching
      ? "Searching…"
      : searchOutcome
        ? `${searchOutcome.total.toLocaleString()} result${searchOutcome.total === 1 ? "" : "s"}`
        : null
    : inCollection && view.kind === "collection"
      ? `${viewTotal.toLocaleString()} screenshots`
      : null;

  const selectAllTotal = async () => {
    setSelectingAll(true);
    try {
      const ids = inSearch
        ? await api.searchIds(activeQuery, searchOutcome?.total ?? SEARCH_PAGE_SIZE)
        : inCollection && view.kind === "collection"
          ? await api.collectionItemIds(view.id)
          : await api.allScreenshotIds();
      sel.selectAll(ids);
    } catch (e) {
      setOrganizeError(String(e));
    } finally {
      setSelectingAll(false);
    }
  };

  const emptyLibrary = !inSearch && !inCollection && rows.length === 0 && !loading;
  const noResults =
    inSearch && !searching && !searchError && (searchOutcome?.rows.length ?? 0) === 0;
  const emptyCollection =
    inCollection && collectionItems.length === 0;

  /* ---- filter pill state derived from the query ---- */
  const afterVal = getToken(query, "after");
  const todayIso = isoDay(new Date());
  const dateOpt = afterVal === todayIso ? "today" : afterVal === daysAgo(7) ? "7d" : afterVal === daysAgo(30) ? "30d" : afterVal ? "custom" : "all";
  const typeVal = (getToken(query, "type") ?? "").toLowerCase();
  const typeOpt = !typeVal ? "any" : TYPE_OPTIONS.includes(typeVal) ? typeVal : "custom";
  const tagVal = getToken(query, "tag");
  const collVal = getToken(query, "collection");
  const appVal = getToken(query, "app") ?? "";
  const starredOn = hasFlag(query, "is:starred");

  const viewTitle =
    view.kind === "collection" ? view.name :
    view.kind === "timeline" ? "Timeline" :
    view.kind === "duplicates" ? "Duplicates" :
    view.kind === "bursts" ? "Bursts" :
    view.kind === "settings" ? "Settings" : "All Screenshots";
  const starredActive = hasFlag(activeQuery, "is:starred");
  const isStarredView = activeQuery.trim() === "is:starred";

  const goAll = () => { setQuery(""); setView({ kind: "all" }); };

  return (
    <div className="library-shell">
      <aside className="sidebar" aria-label="Organize">
        <div className="brand">
          <span className="brand-mark"><Icons.layers size={15} /></span>
          <span>
            <div className="brand-name">Screenshot Memory</div>
            <div className="brand-sub">Find anything in your screenshots</div>
          </span>
        </div>

        <nav className="side-section" aria-label="Views">
          <ul className="side-list">
            <li>
              <button
                className={`side-item${view.kind === "all" && !starredActive && !inSearch ? " active" : ""}`}
                onClick={goAll}
              >
                <span className="side-ic"><Icons.grid size={16} /></span>
                <span className="side-label">All Screenshots</span>
                <CountBadge n={totalAll} />
              </button>
            </li>
            <li>
              <button
                className={`side-item${starredActive ? " active" : ""}`}
                onClick={() => { setView({ kind: "all" }); setQuery("is:starred"); }}
                title="Search is:starred"
              >
                <span className="side-ic"><Icons.star size={16} /></span>
                <span className="side-label">Starred</span>
                {!!starredCount && <CountBadge n={starredCount} />}
              </button>
            </li>
            {NAV.slice(1).map((n) => {
              const Icon = Icons[n.icon];
              return (
                <li key={n.kind}>
                  <button
                    className={`side-item${view.kind === n.kind && !inSearch ? " active" : ""}`}
                    onClick={() => { setQuery(""); setView({ kind: n.kind } as View); }}
                    title={n.title}
                  >
                    <span className="side-ic"><Icon size={16} /></span>
                    <span className="side-label">{n.label}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </nav>

        <div className="side-section">
          <h4>Folders</h4>
          {dirs.length > 0 && (
            <ul className="side-list">
              {dirs.map((d) => {
                const base = d.path.split(/[/\\]/).filter(Boolean).pop() ?? d.path;
                const active =
                  !inSearch &&
                  (getToken(query, "dir") === d.path ||
                    getToken(query, "folder") === d.path ||
                    getToken(query, "path") === d.path);
                return (
                  <li key={d.id} className="side-row" title={d.path}>
                    <button
                      className={`side-item${active ? " active" : ""}`}
                      onClick={() => {
                        setView({ kind: "all" });
                        setQuery(`dir:"${d.path}"`);
                      }}
                      title={`Show screenshots under ${d.path}`}
                    >
                      <span className="side-ic"><Icons.folder size={16} /></span>
                      <span className="side-label">{base}</span>
                      {!d.enabled && <span className="side-count">off</span>}
                    </button>
                    <span className="side-ops">
                      <button
                        className="icon-btn"
                        title={`Stop watching ${d.path}`}
                        aria-label={`Stop watching ${d.path}`}
                        onClick={() => void removeFolder(d)}
                      >
                        ✕
                      </button>
                    </span>
                  </li>
                );
              })}
            </ul>
          )}
          <button className="ghost-row" onClick={() => void addFolder()} title="Watch a new folder for screenshots">
            <Icons.plus size={14} /> Add folder
          </button>
        </div>

        <div className="side-section">
          <h4>Tags</h4>
          {tags.length === 0 ? (
            <p className="muted small" style={{ margin: "0 0 0 8px" }}>No tags yet — add one in the detail view.</p>
          ) : (
            <ul className="side-list">
              {tags.map((t) => (
                <li key={t.name}>
                  <button
                    className="side-item"
                    onClick={() => { setView({ kind: "all" }); setQuery(`tag:"${t.name}"`); }}
                    title={`Search tag:${t.name}`}
                  >
                    <span className="dot-color" style={{ background: "var(--accent)" }} />
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
                      <span className="side-ic"><Icons.layers size={16} /></span>
                      <span className="side-label">{c.name}</span>
                      {c.item_count > 0 && <span className="side-count">{c.item_count}</span>}
                    </button>
                    <span className="side-ops">
                      <button
                        className="icon-btn"
                        title={`Rename ${c.name}`}
                        aria-label={`Rename ${c.name}`}
                        onClick={() => { setRenamingId(c.id); setRenameDraft(c.name); }}
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
              placeholder="+ New collection"
              aria-label="New collection name"
              value={newCollection}
              onChange={(e) => setNewCollection(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") createCollection(); }}
            />
          </div>
        </div>

        {organizeError && <p className="error small">{organizeError}</p>}

        <div className="side-footer">
          <div className="health-card">
            <span className={`health-dot${appState?.indexing ? " busy" : ""}`} aria-hidden="true" />
            <span>
              <div className="health-line1">{appState?.indexing ? "Indexing…" : "Index up to date"}</div>
              <div className="health-line2">
                Watching {watchedCount} folder{watchedCount === 1 ? "" : "s"}
              </div>
            </span>
          </div>
          <div className="theme-row">
            <span className="side-ic">{theme === "dark" ? <Icons.moon size={16} /> : <Icons.sun size={16} />}</span>
            <span className="side-label">Dark mode</span>
            <Toggle checked={theme === "dark"} onChange={onToggleTheme} label="Toggle dark mode" />
          </div>
        </div>
      </aside>

      <div className="library-main">
        <header className="toolbar-sticky">
          <div className="header-row">
            <SearchBar
              ref={searchRef}
              placeholder={SEARCH_HINT}
              aria-label="Search screenshots"
              kbd="Ctrl K"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            {!inSpecial && gridRows.length > 0 && (
              <button
                className="cull-btn"
                onClick={() => setCulling(true)}
                title="Keyboard triage: → keep, x trash, u undo"
              >
                <Icons.keyboard size={14} /> Cull
              </button>
            )}
          </div>

          {!inSpecial && (
            <div className="toolbar-row" role="toolbar" aria-label="Filters">
              <Dropdown
                ariaLabel="Date filter"
                value={dateOpt}
                onChange={(v) => {
                  if (v === "all") setQuery(setToken(query, "after", null));
                  else if (v === "today") setQuery(setToken(query, "after", todayIso));
                  else if (v === "7d") setQuery(setToken(query, "after", daysAgo(7)));
                  else if (v === "30d") setQuery(setToken(query, "after", daysAgo(30)));
                }}
                options={[
                  { value: "all", label: "All time" },
                  { value: "today", label: "Today" },
                  { value: "7d", label: "Last 7 days" },
                  { value: "30d", label: "Last 30 days" },
                  ...(dateOpt === "custom" && afterVal ? [{ value: "custom" as string, label: `After ${afterVal}` }] : []),
                ]}
              />
              <Dropdown
                ariaLabel="Type filter"
                value={typeOpt}
                onChange={(v) => setQuery(setToken(query, "type", v === "any" ? null : v))}
                options={[
                  { value: "any", label: "Any type" },
                  ...TYPE_OPTIONS.map((t) => ({ value: t, label: t })),
                  ...(typeOpt === "custom" && typeVal ? [{ value: "custom" as string, label: typeVal }] : []),
                ]}
              />
              <Dropdown
                ariaLabel="Tag filter"
                value={tagVal ?? "any"}
                onChange={(v) => setQuery(setToken(query, "tag", v === "any" ? null : v))}
                options={[
                  { value: "any", label: "Tags" },
                  ...tags.map((t) => ({ value: t.name, label: `${t.name} · ${t.count}` })),
                  ...(tagVal && !tags.some((t) => t.name === tagVal) ? [{ value: tagVal, label: tagVal }] : []),
                ]}
              />
              <Dropdown
                ariaLabel="Collection filter"
                value={collVal ?? "any"}
                onChange={(v) => setQuery(setToken(query, "collection", v === "any" ? null : v))}
                options={[
                  { value: "any", label: "Collections" },
                  ...collections.map((c) => ({ value: c.name, label: `${c.name} · ${c.item_count}` })),
                  ...(collVal && !collections.some((c) => c.name === collVal) ? [{ value: collVal, label: collVal }] : []),
                ]}
              />
              <input
                className={`filter-input${appVal ? " has-value" : ""}`}
                placeholder="App: any"
                aria-label="Source app filter"
                title="Filter by source app (app:)"
                value={appVal}
                onChange={(e) => setQuery(setToken(query, "app", e.target.value.trim() || null))}
                onKeyDown={(e) => e.stopPropagation()}
              />
              <button
                className={`dd-trigger${starredOn ? " dd-active" : ""}`}
                onClick={() => setQuery(toggleFlag(query, "is:starred"))}
                aria-pressed={starredOn}
                title="Toggle is:starred"
              >
                <Icons.star size={13} /> Starred
              </button>
              {query && (
                <button className="link-btn" onClick={() => setQuery("")}>Clear</button>
              )}
            </div>
          )}

        </header>

        {!inSpecial && (
          <div className="page-head">
            <div>
              <h1 className="page-title">{inSearch ? (isStarredView ? "Starred" : "Search results") : viewTitle}</h1>
              {pageSub && <p className="page-sub">{pageSub}</p>}
            </div>
            <div className="page-head-actions">
              <Button
                size="sm"
                variant={selectMode ? "secondary" : "ghost"}
                onClick={() => (selectMode ? exitSelectMode() : setSelectMode(true))}
                title="Select screenshots for bulk actions"
              >
                {selectMode ? "Cancel" : "Select"}
              </Button>
              <span className="view-seg" role="group" aria-label="View">
                <button className={viewMode === "grid" ? "on" : ""} onClick={() => setViewMode("grid")} title="Grid view" aria-label="Grid view" aria-pressed={viewMode === "grid"}>
                  <Icons.grid size={15} />
                </button>
                <button className={viewMode === "list" ? "on" : ""} onClick={() => setViewMode("list")} title="List view" aria-label="List view" aria-pressed={viewMode === "list"}>
                  <Icons.list size={15} />
                </button>
              </span>
            </div>
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
              accent={accent}
              onAccentChange={onAccentChange}
              customHex={customHex}
              onCustomAccent={onCustomAccent}
            />
          )
        ) : searchError ? (
          <EmptyState title="Search failed" body={<span className="error">{searchError}</span>} />
        ) : noResults ? (
          <EmptyState
            title="No screenshots found"
            body="No screenshots match these filters."
            action={<Button onClick={() => setQuery("")}>Clear filters</Button>}
          />
        ) : emptyLibrary ? (
          <EmptyState
            title="No screenshots yet"
            body="Add a screenshot folder from the sidebar and we'll build your searchable visual memory automatically."
          />
        ) : emptyCollection ? (
          <EmptyState
            title="Empty collection"
            body="Open a screenshot and add it via “In collections”."
          />
        ) : (
          <>
            {selectMode && (
              <BulkBar
                ids={[...sel.selected]}
                collections={collections}
                onDone={afterBulk}
                onError={setOrganizeError}
                selectAllLabel={inSearch ? "Select all results" : "Select all"}
                selectingAll={selectingAll}
                onSelectAll={() => void selectAllTotal()}
                onCancel={exitSelectMode}
              />
            )}
            {viewMode === "grid" ? (
              <div className={`shot-grid${selectMode ? " selecting" : ""}`}>
                {gridRows.map((r) => (
                  <ScreenshotCard
                    key={r.id}
                    row={r}
                    thumbUrl={thumbs.get(r.id)}
                    selected={sel.selected.has(r.id)}
                    onOpen={(id) => setDetailId(id)}
                    onToggleSelect={toggleSel}
                    onToggleStar={toggleStar}
                  />
                ))}
              </div>
            ) : (
              <div className={`shot-list${selectMode ? " selecting" : ""}`}>
                {gridRows.map((r) => (
                  <div
                    key={r.id}
                    className={`shot-row${sel.selected.has(r.id) ? " selected" : ""}`}
                    title={r.filename}
                    onClick={() => setDetailId(r.id)}
                  >
                    <span onClick={(e) => e.stopPropagation()}>
                      <input
                        type="checkbox"
                        checked={sel.selected.has(r.id)}
                        onChange={() => toggleSel(r.id)}
                        aria-label={`Select ${r.filename}`}
                      />
                    </span>
                    <span className="shot-row-thumb">
                      {thumbs.get(r.id) ? (
                        <img src={thumbs.get(r.id)} alt="" loading="lazy" />
                      ) : (
                        <Icons.image size={18} />
                      )}
                    </span>
                    <span className="shot-row-main">
                      <span className="shot-name">{displayName(r.filename)}</span>
                      <span className="shot-date">
                        {r.created_ts ? new Date(r.created_ts * 1000).toLocaleString(undefined, { year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }) : ""}
                        {r.status !== "available" ? ` • ${r.status}` : ""}
                      </span>
                    </span>
                    <IconButton
                      icon="star"
                      label={r.starred ? "Unstar" : "Star"}
                      className={r.starred ? "lit" : ""}
                      onClick={(e) => { e.stopPropagation(); toggleStar(r.id, !r.starred); }}
                    />
                  </div>
                ))}
              </div>
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
                <Button onClick={loadMoreNext} disabled={loading}>
                  {loading ? "Loading…" : `Load more (${gridRows.length} shown)`}
                </Button>
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
            onPrev={(() => {
              const i = gridRows.findIndex((r) => r.id === detailId);
              return i > 0 ? () => setDetailId(gridRows[i - 1].id) : undefined;
            })()}
            onNext={(() => {
              const i = gridRows.findIndex((r) => r.id === detailId);
              return i >= 0 && i < gridRows.length - 1 ? () => setDetailId(gridRows[i + 1].id) : undefined;
            })()}
            position={(() => {
              const i = gridRows.findIndex((r) => r.id === detailId);
              return i >= 0 ? `${i + 1} / ${gridRows.length}` : undefined;
            })()}
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
        <ToastStack toasts={toasts} onClose={dismissToast} onPause={pauseToast} onResume={resumeToast} />
      </div>
    </div>
  );
}
