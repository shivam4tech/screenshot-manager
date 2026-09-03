import { useState } from "react";
import { api, type CollectionInfo } from "../api";

/** Selection state shared by every grid (library, bursts, timeline...). */
export function useSelection() {
  const [selected, setSelected] = useState<Set<number>>(new Set());

  const toggle = (id: number) =>
    setSelected((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const selectAll = (ids: number[]) => setSelected(new Set(ids));
  const clear = () => setSelected(new Set());
  const remove = (ids: number[]) =>
    setSelected((s) => {
      const next = new Set(s);
      for (const id of ids) next.delete(id);
      return next;
    });

  return { selected, toggle, selectAll, clear, remove };
}

export type Selection = ReturnType<typeof useSelection>;

/**
 * Floating bulk bar: collect (existing or new collection), tag, star, and
 * trash the current selection. `onDone(removedIds)` lets the parent drop
 * deleted rows and refresh counts; `removedIds` is empty for non-deletes.
 */
export function BulkBar({
  ids,
  collections,
  onDone,
  onError,
}: {
  ids: number[];
  collections: CollectionInfo[];
  onDone: (removedIds: number[]) => void;
  onError: (msg: string) => void;
}) {
  const [target, setTarget] = useState("");
  const [newName, setNewName] = useState("");
  const [tag, setTag] = useState("");
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  const run = async (label: string, fn: () => Promise<number[]>) => {
    setBusy(true);
    setNote(null);
    try {
      const removed = await fn();
      setNote(label);
      onDone(removed);
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const collect = () =>
    run("", async () => {
      let cid = Number(target);
      let cname = collections.find((c) => c.id === cid)?.name ?? "";
      if (!cid) {
        const name = newName.trim();
        if (!name) throw new Error("pick a collection or type a new name");
        const c = await api.createCollection(name);
        cid = c.id;
        cname = c.name;
        setNewName("");
        setTarget(String(cid));
      }
      const n = await api.addManyToCollection(cid, ids);
      setNote(`Added ${n} to “${cname}”.`);
      return [];
    });

  const tagAll = () =>
    run("", async () => {
      const name = tag.trim();
      if (!name) throw new Error("type a tag first");
      await Promise.all(ids.map((id) => api.addTag(id, name)));
      setTag("");
      setNote(`Tagged ${ids.length} with “${name}”.`);
      return [];
    });

  const starAll = () =>
    run(`Starred ${ids.length}.`, async () => {
      await Promise.all(ids.map((id) => api.setStarred(id, true)));
      return [];
    });

  const trashAll = () => {
    if (
      !window.confirm(
        `Move ${ids.length} screenshot${ids.length === 1 ? "" : "s"} to the trash?\n\nFiles go to the OS trash (recoverable); their records stay in the library as missing.`
      )
    )
      return;
    void run("", async () => {
      const s = await api.deleteScreenshots(ids);
      const gone = ids.filter((id) => !s.failed.some((f) => f.id === id));
      const bits = [`${s.trashed} trashed`];
      if (s.already_missing > 0) bits.push(`${s.already_missing} already gone`);
      if (s.failed.length > 0) bits.push(`${s.failed.length} failed`);
      setNote(`${bits.join(", ")}.`);
      return gone;
    });
  };

  return (
    <div className="bulk-bar" role="toolbar" aria-label="Bulk actions">
      <span className="bulk-count">{ids.length} selected</span>
      <select
        value={target}
        disabled={busy}
        onChange={(e) => setTarget(e.target.value)}
        aria-label="Choose collection"
      >
        <option value="">Collect into…</option>
        {collections.map((c) => (
          <option key={c.id} value={c.id}>
            {c.name} ({c.item_count})
          </option>
        ))}
      </select>
      <span className="bulk-new-collection">
        <input
          type="text"
          placeholder="or new…"
          aria-label="New collection name"
          value={newName}
          disabled={busy}
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void collect();
          }}
        />
        <button disabled={busy} onClick={() => void collect()}>
          Add
        </button>
      </span>
      <span className="bulk-new-collection">
        <input
          type="text"
          placeholder="tag…"
          aria-label="Tag for selection"
          value={tag}
          disabled={busy}
          onChange={(e) => setTag(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void tagAll();
          }}
        />
        <button disabled={busy} onClick={() => void tagAll()}>
          Tag
        </button>
      </span>
      <button disabled={busy} onClick={() => void starAll()} title="Star all selected">
        ★
      </button>
      <button
        disabled={busy}
        className="danger"
        onClick={trashAll}
        title="Move selected to trash"
      >
        Delete
      </button>
      {note && (
        <span className="muted small" role="status">
          {note}
        </span>
      )}
    </div>
  );
}
