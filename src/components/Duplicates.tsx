import { useCallback, useEffect, useState } from "react";
import {
  api,
  thumbnailUrl,
  type CollectionInfo,
  type DuplicateGroup,
} from "../api";

const THRESHOLDS = [4, 8, 12];

/**
 * Duplicate manager: exact (byte-identical) and similar (perceptual) groups
 * for review and bulk organization. Never deletes files — actions are tag,
 * star, and add-to-collection across the whole group.
 */
export default function Duplicates({ onOpenDetail }: { onOpenDetail: (id: number) => void }) {
  const [exact, setExact] = useState<DuplicateGroup[]>([]);
  const [similar, setSimilar] = useState<DuplicateGroup[]>([]);
  const [threshold, setThreshold] = useState(8);
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

  const bulk = async (label: string, fn: () => Promise<unknown>) => {
    setNote(null);
    setError(null);
    try {
      await fn();
      setNote(label);
    } catch (e) {
      setError(String(e));
    }
  };

  const renderGroup = (g: DuplicateGroup, gi: number) => {
    const key = `${g.kind}:${gi}`;
    const draft = tagDrafts[key] ?? "";
    return (
      <section className="dup-group" key={key}>
        <header className="dup-head">
          <span className={`dup-kind dup-${g.kind}`}>
            {g.kind === "exact" ? "Exact" : "Similar"}
          </span>
          <span className="muted small">
            {g.items.length} shots ·{" "}
            <span className="mono">{g.key.slice(0, 12)}…</span>
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
              </div>
              <figcaption>{r.filename}</figcaption>
            </figure>
          ))}
        </div>
        <div className="dup-actions">
          <button
            className="link-btn"
            onClick={() =>
              bulk(`Starred ${g.items.length} shots.`, () =>
                Promise.all(g.items.map((r) => api.setStarred(r.id, true))).then(() => {})
              )
            }
          >
            ★ Star all
          </button>
          <span className="dup-tag-add">
            <input
              type="text"
              placeholder="tag all…"
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
          {collections.length > 0 && (
            <select
              className="dup-collect"
              defaultValue=""
              aria-label="Add whole group to collection"
              onChange={(e) => {
                const cid = Number(e.target.value);
                e.target.value = "";
                if (!cid) return;
                const name = collections.find((c) => c.id === cid)?.name ?? "";
                void bulk(`Added ${g.items.length} shots to “${name}”.`, () =>
                  Promise.all(
                    g.items.map((r) => api.addToCollection(cid, r.id))
                  ).then(() => {})
                );
              }}
            >
              <option value="">+ collect all…</option>
              {collections.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          )}
        </div>
      </section>
    );
  };

  return (
    <div className="duplicates">
      <div className="dup-toolbar">
        <h3 className="dup-title">Duplicate review</h3>
        <label className="muted small">
          Similarity{" "}
          <select
            value={threshold}
            onChange={(e) => setThreshold(Number(e.target.value))}
            aria-label="Similarity threshold"
          >
            {THRESHOLDS.map((t) => (
              <option key={t} value={t}>
                ≤ {t} bits
              </option>
            ))}
          </select>
        </label>
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
        <div className="empty-state">
          <h2>No duplicates found.</h2>
          <p>Byte-identical and visually similar shots will group here.</p>
        </div>
      ) : (
        <>
          {exact.length > 0 && (
            <>
              <h4>
                Exact duplicates{" "}
                <span className="side-count">{exact.length} groups</span>
              </h4>
              {exact.map((g, i) => renderGroup(g, i))}
            </>
          )}
          {similar.length > 0 && (
            <>
              <h4>
                Similar shots{" "}
                <span className="side-count">{similar.length} groups</span>
              </h4>
              {similar.map((g, i) => renderGroup(g, i + exact.length))}
            </>
          )}
        </>
      )}
    </div>
  );
}
