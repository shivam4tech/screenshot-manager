import { useState } from "react";
import { api, type CollectionInfo } from "../api";
import { Dropdown, useConfirm } from "./ui";

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
 * Contextual selection toolbar: count, select-all, collect (existing or new
 * collection), tag, star, trash, and cancel. Rendered above the grid only
 * while selection mode is active. `onDone(removedIds)` lets the parent drop
 * deleted rows and refresh counts; `removedIds` is empty for non-deletes.
 */
export function BulkBar({
  ids,
  collections,
  onDone,
  onError,
  selectAllLabel,
  selectingAll,
  onSelectAll,
  onCancel,
  trashHotkeyRef,
}: {
  ids: number[];
  collections: CollectionInfo[];
  onDone: (removedIds: number[]) => void;
  onError: (msg: string) => void;
  selectAllLabel: string;
  selectingAll: boolean;
  onSelectAll: () => void;
  onCancel: () => void;
  /** Lets parents trigger the trash action from a Delete hotkey. */
  trashHotkeyRef?: { current: (() => void) | null };
}) {
  const [target, setTarget] = useState("");
  const [newName, setNewName] = useState("");
  const [tag, setTag] = useState("");
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const { confirm, confirmNode } = useConfirm();

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

  const trashAll = async () => {
    const ok = await confirm({
      title: `Move ${ids.length} screenshot${ids.length === 1 ? "" : "s"} to the trash?`,
      body: "Files go to the OS trash (recoverable); their records stay in the library as missing.",
      confirmLabel: "Move to trash",
      danger: true,
    });
    if (!ok) return;
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

  // Expose the trash action so parents can bind it to the Delete hotkey.
  if (trashHotkeyRef) trashHotkeyRef.current = trashAll;

  return (
    <>
    <div className="bulk-bar" role="toolbar" aria-label="Bulk actions">
      {ids.length === 0 ? (
        <button className="btn btn-sm btn-ghost" disabled={busy || selectingAll} onClick={onSelectAll} title="Select every screenshot in this view">
          {selectingAll ? "Selecting…" : selectAllLabel}
        </button>
      ) : (
      <>
      <span className="bulk-count">{ids.length} selected</span>
      <button className="btn btn-sm btn-ghost" disabled={busy || selectingAll} onClick={onSelectAll} title="Select every screenshot in this view">
        {selectingAll ? "Selecting…" : selectAllLabel}
      </button>
      <span className="bulk-sep" aria-hidden="true" />
      <Dropdown
        value={target}
        active={false}
        disabled={busy}
        ariaLabel="Choose collection"
        placeholder="Collect into…"
        options={collections.map((c) => ({ value: String(c.id), label: `${c.name} · ${c.item_count}` }))}
        onChange={(v) => setTarget(v)}
      />
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
        <button className="btn btn-sm" disabled={busy} onClick={() => void collect()}>
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
        <button className="btn btn-sm" disabled={busy} onClick={() => void tagAll()}>
          Tag
        </button>
      </span>
      <button className="btn btn-sm" disabled={busy} onClick={() => void starAll()} title="Star all selected">
        ★ Star
      </button>
      <button
        disabled={busy}
        className="btn btn-sm btn-danger"
        onClick={trashAll}
        title="Move selected to trash"
      >
        Delete
      </button>
      <button
        className="iconbtn"
        disabled={busy}
        onClick={onCancel}
        title="Cancel selection"
        aria-label="Cancel selection"
      >
        ✕
      </button>
      {note && (
        <span className="muted small" role="status">
          {note}
        </span>
      )}
      </>
      )}
    </div>
    {confirmNode}
    </>
  );
}
