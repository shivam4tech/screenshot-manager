import { useCallback, useEffect, useState } from "react";
import {
  api,
  thumbnailUrl,
  type CollectionInfo,
  type DuplicateGroup,
} from "../api";
import { Icons } from "./icons";
import { Button, Dropdown, EmptyState, useConfirm, type ToastAction } from "./ui";
import { displayName } from "./ScreenshotCard";

const THRESHOLDS = [4, 8, 12];

/**
 * Duplicate manager: exact (byte-identical) and similar (perceptual) groups
 * for review and bulk organization. Per-group actions cover tagging,
 * starring, collecting, and keep-newest trash; a header action applies
 * keep-newest across every exact group at once (never similar groups).
 */
export default function Duplicates({
  onOpenDetail,
  onChanged,
  onNotify,
}: {
  onOpenDetail: (id: number) => void;
  onChanged: () => void;
  onNotify?: (msg: string, action?: ToastAction) => void;
}) {
  const [exact, setExact] = useState<DuplicateGroup[]>([]);
  const [similar, setSimilar] = useState<DuplicateGroup[]>([]);
  const [threshold, setThreshold] = useState(8);
  const [tab, setTab] = useState<"exact" | "similar">("exact");
  const [collections, setCollections] = useState<CollectionInfo[]>([]);
  const [thumbs, setThumbs] = useState<Map<number, string>>(new Map());
  const [tagDrafts, setTagDrafts] = useState<Record<string, string>>({});
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const resolveThumbs = useCallback(async (groups: DuplicateGroup[]) => {
    const fresh = new Map<number, string>();
    for (const g of groups) {
      for (const row of g.items) {
        if (fresh.has(row.id)) continue;
        const url = await thumbnailUrl(row.content_hash, 256);
        if (url) fresh.set(row.id, url);
      }
    }
    setThumbs((m) => new Map([...m, ...fresh]));
  }, []);

  const reload = useCallback(
    async (maxDistance: number) => {
      setLoading(true);
      setError(null);
      try {
        const [e, s, c] = await Promise.all([
          api.exactDuplicateGroups(),
          api.similarGroups(maxDistance),
          api.listCollections(),
        ]);
        setExact(e);
        setSimilar(s);
        setCollections(c);
        void resolveThumbs([...e, ...s]);
      } catch (err) {
        setError(String(err));
      } finally {
        setLoading(false);
      }
    },
    [resolveThumbs]
  );

  useEffect(() => {
    reload(threshold);
  }, [reload, threshold]);

  const { confirm, confirmNode } = useConfirm();

  const bulk = async (label: string, fn: () => Promise<unknown>) => {
    setNote(null);
    setError(null);
    try {
      await fn();
      setNote(label);
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const trashItems = (ids: number[], label: string) =>
    bulk("", async () => {
      const s = await api.deleteScreenshots(ids);
      const bits = [`${s.trashed} trashed`];
      if (s.already_missing > 0) bits.push(`${s.already_missing} already gone`);
      if (s.failed.length > 0) bits.push(`${s.failed.length} failed`);
      setNote(`${label}: ${bits.join(", ")}. Records kept as missing.`);
      await reload(threshold);
    });

  const trashGroupExceptNewest = async (g: DuplicateGroup) => {
    if (g.items.length < 2) return;
    const ok = await confirm({
      title: `Move ${g.items.length - 1} older ${g.items.length - 1 === 1 ? "copy" : "copies"} to the trash?`,
      body: "Keeps only the newest. Files go to the OS trash (recoverable); records stay as missing.",
      confirmLabel: "Keep newest only",
      danger: true,
    });
    if (!ok) return;
    // Items are newest-first, so everything past the first goes.
    void trashItems(
      g.items.slice(1).map((r) => r.id),
      "Duplicates cleared"
    );
  };

  /** Keep the newest copy in every exact group (never similar groups). */
  const keepNewestEverywhere = async () => {
    const groups = exact.filter((g) => g.items.length > 1);
    if (groups.length === 0) return;
    const ids = groups.flatMap((g) => g.items.slice(1).map((r) => r.id));
    const ok = await confirm({
      title: `Keep newest in ${groups.length} duplicate group${groups.length === 1 ? "" : "s"}?`,
      body: `${ids.length} older ${ids.length === 1 ? "copy" : "copies"} will move to the OS Trash.\nKeep strategy: newest copy.\n\nSimilar screenshots are never touched by this action.`,
      confirmLabel: `Trash ${ids.length}`,
      danger: true,
    });
    if (!ok) return;
    setError(null);
    try {
      const s = await api.deleteScreenshots(ids);
      const gone = ids.filter((id) => !s.failed.some((f) => f.id === id));
      const bits = [`${s.trashed} trashed`];
      if (s.already_missing > 0) bits.push(`${s.already_missing} already gone`);
      if (s.failed.length > 0) bits.push(`${s.failed.length} failed`);
      setNote(`Kept newest everywhere: ${bits.join(", ")}.`);
      await reload(threshold);
      onChanged();
      if (gone.length > 0) {
        onNotify?.(
          `${gone.length} duplicate ${gone.length === 1 ? "copy" : "copies"} moved to Trash.`,
          {
            label: "Undo",
            fn: () => {
              void api
                .restoreScreenshots(gone)
                .then(() => {
                  reload(threshold).catch(() => {});
                  onChanged();
                })
                .catch((e) => setError(String(e)));
            },
          }
        );
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const renderGroup = (g: DuplicateGroup, gi: number) => {
    const key = `${g.kind}:${gi}`;
    const draft = tagDrafts[key] ?? "";
    const isExact = g.kind === "exact";
    return (
      <section className="dup-group" key={key}>
        <header className="dup-head">
          <span className="dup-head-main">
            <span className={`dup-kind dup-${g.kind}`}>
              {isExact ? "Exact match" : "Similar"}
            </span>
            <span className="muted small">
              {g.items.length} screenshots ·{" "}
              <span className="mono">{g.key.slice(0, 12)}…</span>
            </span>
          </span>
          <span className="dup-head-actions">
            {g.items.length > 1 && (
              <Button
                size="sm"
                variant="ghost"
                onClick={() => trashGroupExceptNewest(g)}
                title="Trash every copy but the newest (recoverable via OS trash)"
              >
                Keep newest only
              </Button>
            )}
            <Button
              size="sm"
              variant="ghost"
              icon="star"
              onClick={() =>
                bulk(`Starred ${g.items.length} shots.`, () =>
                  Promise.all(g.items.map((r) => api.setStarred(r.id, true))).then(() => {})
                )
              }
            >
              Star all
            </Button>
            {collections.length > 0 && (
              <Dropdown
                value=""
                active={false}
                ariaLabel="Add whole group to collection"
                placeholder="+ Collect"
                options={collections.map((c) => ({ value: String(c.id), label: c.name }))}
                onChange={(v) => {
                  const cid = Number(v);
                  if (!cid) return;
                  const name = collections.find((c) => c.id === cid)?.name ?? "";
                  void bulk(`Added ${g.items.length} shots to “${name}”.`, () =>
                    Promise.all(
                      g.items.map((r) => api.addToCollection(cid, r.id))
                    ).then(() => {})
                  );
                }}
              />
            )}
          </span>
        </header>
        <div className="dup-items">
          {g.items.map((r) => (
            <figure
              key={r.id}
              className="cell clickable dup-cell"
              title={`${r.filename}\n${r.path}`}
              onClick={() => onOpenDetail(r.id)}
            >
              <div className="thumb-box">
                {thumbs.has(r.id) ? (
                  <img src={thumbs.get(r.id)} alt={r.filename} loading="lazy" />
                ) : (
                  <div className="thumb-placeholder" aria-hidden="true" />
                )}
                {r.starred && (
                  <span className="star-badge" title="Starred">
                    ★
                  </span>
                )}
                <button
                  className="cell-remove"
                  title="Move to trash (record kept as missing)"
                  aria-label={`Delete ${r.filename}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    void trashItems([r.id], "Deleted");
                  }}
                >
                  ✕
                </button>
              </div>
              <figcaption>{displayName(r.filename)}</figcaption>
            </figure>
          ))}
        </div>
        <div className="dup-actions">
          <span className="dup-tag-add">
            <input
              type="text"
              placeholder="+ Tag all…"
              aria-label="Tag whole group"
              value={draft}
              onChange={(e) =>
                setTagDrafts((d) => ({ ...d, [key]: e.target.value }))
              }
              onKeyDown={(e) => {
                if (e.key !== "Enter" || !draft.trim()) return;
                const name = draft.trim();
                setTagDrafts((d) => ({ ...d, [key]: "" }));
                void bulk(`Tagged ${g.items.length} shots “${name}”.`, () =>
                  Promise.all(g.items.map((r) => api.addTag(r.id, name))).then(() => {})
                );
              }}
            />
          </span>
          {g.items.length > 1 && (
            <button
              className="link-btn danger-text"
              onClick={() => trashGroupExceptNewest(g)}
              title="Trash every copy but the newest (recoverable via OS trash)"
            >
              <Icons.trash size={12} /> Move duplicates to trash
            </button>
          )}
        </div>
      </section>
    );
  };

  const groups = tab === "exact" ? exact : similar;

  return (
    <div className="duplicates">
      <div className="page-head">
        <div>
          <h1 className="page-title">Duplicate review</h1>
          <p className="page-sub">Find and manage similar screenshots</p>
        </div>
      </div>
      <div className="dup-toolbar">
        <div className="dup-tabs" role="tablist" aria-label="Duplicate kinds">
          <button
            role="tab"
            aria-selected={tab === "exact"}
            className={`dup-tab${tab === "exact" ? " on" : ""}`}
            onClick={() => setTab("exact")}
          >
            Exact duplicates <span className="side-count">{exact.length}</span>
          </button>
          <button
            role="tab"
            aria-selected={tab === "similar"}
            className={`dup-tab${tab === "similar" ? " on" : ""}`}
            onClick={() => setTab("similar")}
          >
            Similar <span className="side-count">{similar.length}</span>
          </button>
        </div>
        <span className="toolbar-group">
          {exact.some((g) => g.items.length > 1) && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void keepNewestEverywhere()}
              title="Trash every older copy in all exact groups (similar groups untouched)"
            >
              Keep newest everywhere
            </Button>
          )}
          <Dropdown
            ariaLabel="Similarity threshold"
            prefix="Similarity:"
            value={String(threshold)}
            onChange={(v) => setThreshold(Number(v))}
            options={THRESHOLDS.map((t) => ({ value: String(t), label: `≤ ${t} bits` }))}
          />
        </span>
      </div>
      {note && (
        <p className="muted small" role="status">
          {note}
        </p>
      )}
      {error && <p className="error">{error}</p>}
      {loading ? (
        <p className="muted">Scanning for duplicates…</p>
      ) : exact.length === 0 && similar.length === 0 ? (
        <EmptyState
          title="No duplicate screenshots detected"
          body="Byte-identical and visually similar shots will group here."
        />
      ) : groups.length === 0 ? (
        <EmptyState
          title={tab === "exact" ? "No exact duplicates" : "No similar shots"}
          body={tab === "exact"
            ? "No byte-identical screenshots found."
            : "Try raising the similarity threshold."}
        />
      ) : (
        groups.map((g, i) => renderGroup(g, tab === "exact" ? i : i + exact.length))
      )}
      {confirmNode}
    </div>
  );
}
