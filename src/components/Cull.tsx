import { useCallback, useEffect, useState } from "react";
import { api, thumbnailUrl } from "../api";

export interface CullItem {
  id: number;
  filename: string;
  content_hash: string | null;
}

/**
 * Cull mode: full-screen rapid triage. Keep (next) or trash each shot with
 * the keyboard; undo restores the last trashed file from the OS trash.
 *
 * Keys: →/space/j next · ←/k previous · x/del trash · u undo · esc done.
 * Trash is recoverable (records stay as missing); undo works in-session.
 */
export default function Cull({
  items,
  onDone,
}: {
  items: CullItem[];
  onDone: (trashedIds: number[]) => void;
}) {
  const [index, setIndex] = useState(0);
  const [trashed, setTrashed] = useState<number[]>([]);
  const [undoStack, setUndoStack] = useState<number[]>([]);
  const [imgUrl, setImgUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [finished, setFinished] = useState(items.length === 0);

  const trashedSet = new Set(trashed);

  const step = useCallback(
    (from: number, dir: 1 | -1): number | null => {
      let i = from + dir;
      while (i >= 0 && i < items.length) {
        if (!trashedSet.has(items[i].id)) return i;
        i += dir;
      }
      return null;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [items, trashed.join(",")]
  );

  // Load the preview for the current item (plus prefetch the next).
  useEffect(() => {
    let alive = true;
    setImgUrl(null);
    const cur = items[index];
    if (!cur) return;
    thumbnailUrl(cur.content_hash, 1024).then((u) => alive && setImgUrl(u));
    const nxt = items[index + 1];
    if (nxt) void thumbnailUrl(nxt.content_hash, 1024);
    return () => {
      alive = false;
    };
  }, [items, index]);

  const trashCurrent = useCallback(async () => {
    const cur = items[index];
    if (!cur || busy) return;
    setBusy(true);
    setError(null);
    try {
      const s = await api.deleteScreenshots([cur.id]);
      if (s.failed.length > 0) {
        setError(s.failed[0].message);
        return;
      }
      setTrashed((t) => [...t, cur.id]);
      setUndoStack((u) => [...u, cur.id]);
      const next = step(index, 1);
      if (next === null) setFinished(true);
      else setIndex(next);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [items, index, busy, step]);

  const undoLast = useCallback(async () => {
    const last = undoStack[undoStack.length - 1];
    if (last === undefined || busy) return;
    setBusy(true);
    setError(null);
    try {
      const s = await api.restoreScreenshots([last]);
      if (s.failed.length > 0) {
        setError(s.failed[0].message);
        return;
      }
      setUndoStack((u) => u.slice(0, -1));
      setTrashed((t) => t.filter((id) => id !== last));
      const pos = items.findIndex((r) => r.id === last);
      setFinished(false);
      if (pos >= 0) setIndex(pos);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [undoStack, busy, items]);

  const goNext = useCallback(() => {
    const next = step(index, 1);
    if (next === null) setFinished(true);
    else setIndex(next);
  }, [index, step]);

  const goPrev = useCallback(() => {
    const prev = step(index, -1);
    if (prev !== null) {
      setFinished(false);
      setIndex(prev);
    }
  }, [index, step]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onDone(trashed);
        return;
      }
      if (finished) {
        if (e.key === "u" || e.key === "U") void undoLast();
        return;
      }
      switch (e.key) {
        case "ArrowRight":
        case " ":
        case "j":
        case "J":
          e.preventDefault();
          goNext();
          break;
        case "ArrowLeft":
        case "k":
        case "K":
          goPrev();
          break;
        case "x":
        case "X":
        case "Delete":
        case "Backspace":
          e.preventDefault();
          void trashCurrent();
          break;
        case "u":
        case "U":
          void undoLast();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [goNext, goPrev, trashCurrent, undoLast, finished, trashed.join(",")]);

  const cur = items[index];
  const remaining = items.length - trashed.length;

  return (
    <div className="cull-backdrop" role="dialog" aria-modal="true" aria-label="Cull mode">
      <div className="cull-top">
        <span className="muted">
          {finished
            ? `Reviewed all ${items.length}`
            : `${index + 1} / ${items.length}`}
          {" · "}
          {trashed.length} trashed
          {undoStack.length > 0 && " (u to undo)"}
        </span>
        <button onClick={() => onDone(trashed)} aria-label="Finish culling">
          Done ✓
        </button>
      </div>

      {error && <p className="error cull-error">{error}</p>}

      <div className="cull-stage">
        {finished ? (
          <div className="empty-state">
            <h2>Reviewed them all.</h2>
            <p>
              {trashed.length} trashed
              {undoStack.length > 0 && " — press u to undo the last one"}. Press
              Done or Esc to return.
            </p>
          </div>
        ) : cur ? (
          <>
            {imgUrl ? (
              <img key={cur.id} src={imgUrl} alt={cur.filename} />
            ) : (
              <div className="thumb-placeholder" aria-hidden="true" />
            )}
            <p className="cull-filename" title={cur.filename}>
              {cur.filename}
            </p>
          </>
        ) : null}
      </div>

      {!finished && (
        <div className="cull-actions">
          <button onClick={goPrev} disabled={busy} title="Previous (←)">
            ← Keep
          </button>
          <button
            onClick={() => void trashCurrent()}
            disabled={busy}
            className="danger"
            title="Trash this file — recoverable, u undoes (x)"
          >
            {busy ? "…" : "✕ Trash (x)"}
          </button>
          <button onClick={goNext} disabled={busy} title="Next (→)">
            Keep →
          </button>
          <button
            onClick={() => void undoLast()}
            disabled={busy || undoStack.length === 0}
            title="Restore last trashed file (u)"
          >
            Undo{remaining < items.length ? ` (${trashed.length})` : ""}
          </button>
        </div>
      )}

      <p className="cull-hints muted small">
        →/space keep · x trash · u undo · esc done — trash goes to the OS trash,
        records stay as missing
      </p>
    </div>
  );
}
