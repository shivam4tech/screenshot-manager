import { forwardRef, useImperativeHandle, useMemo, useState } from "react";
import { Icons } from "./icons";
import type { CollectionInfo, TagInfo } from "../api";

const RECENT_KEY = "shotmemory-recent-searches";
const RECENT_MAX = 8;

export function loadRecents(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY);
    const arr = raw ? (JSON.parse(raw) as unknown) : [];
    return Array.isArray(arr) ? arr.filter((s): s is string => typeof s === "string").slice(0, RECENT_MAX) : [];
  } catch {
    return [];
  }
}

export function saveRecent(q: string) {
  const query = q.trim();
  if (!query) return;
  try {
    const next = [query, ...loadRecents().filter((s) => s !== query)].slice(0, RECENT_MAX);
    localStorage.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    /* ignore */
  }
}

export function clearRecents() {
  try {
    localStorage.removeItem(RECENT_KEY);
  } catch {
    /* ignore */
  }
}

const OPERATORS: Array<{ token: string; hint: string }> = [
  { token: "tag:", hint: "filter by tag" },
  { token: "collection:", hint: "inside a collection" },
  { token: "app:", hint: "source app" },
  { token: "type:", hint: "png, jpg, gif…" },
  { token: "after:", hint: "captured after date" },
  { token: "before:", hint: "captured before date" },
  { token: "is:starred", hint: "starred shots" },
  { token: "is:duplicate", hint: "duplicates" },
  { token: "has:text", hint: "has extracted text" },
];

export interface SuggestItem {
  key: string;
  label: string;
  hint?: string;
  apply: string;
  kind: "recent" | "operator" | "tag" | "collection";
}

/** Build the suggestion list for the current query tail. */
export function computeSuggestions(
  query: string,
  tags: TagInfo[],
  collections: CollectionInfo[],
  recents: string[]
): SuggestItem[] {
  const tail = query.split(/\s+/).pop() ?? "";
  const head = query.slice(0, query.length - tail.length);
  const complete = (token: string) => `${head}${token}${token.endsWith(":") ? "" : " "}`;

  // Completing a tag:/collection: value?
  const tagM = tail.match(/^tag:(.*)$/i);
  if (tagM) {
    const frag = tagM[1].replace(/^"|"$/g, "").toLowerCase();
    return tags
      .filter((t) => t.name.toLowerCase().includes(frag))
      .slice(0, 6)
      .map((t) => ({
        key: `tag:${t.name}`,
        label: `tag:"${t.name}"`,
        hint: `${t.count} shots`,
        apply: complete(`tag:"${t.name}"`),
        kind: "tag" as const,
      }));
  }
  const collM = tail.match(/^collection:(.*)$/i);
  if (collM) {
    const frag = collM[1].replace(/^"|"$/g, "").toLowerCase();
    return collections
      .filter((c) => c.name.toLowerCase().includes(frag))
      .slice(0, 6)
      .map((c) => ({
        key: `collection:${c.name}`,
        label: `collection:"${c.name}"`,
        hint: `${c.item_count} shots`,
        apply: complete(`collection:"${c.name}"`),
        kind: "collection" as const,
      }));
  }

  // Completing an is:/has: value?
  const flagM = tail.match(/^(is|has):(.*)$/i);
  if (flagM) {
    const [, scope, fragRaw] = flagM;
    const frag = fragRaw.toLowerCase();
    const values =
      scope.toLowerCase() === "is"
        ? ["starred", "unstarred", "duplicate"]
        : ["text", "notext"];
    return values
      .filter((v) => v.startsWith(frag))
      .map((v) => ({
        key: `${scope}:${v}`,
        label: `${scope.toLowerCase()}:${v}`,
        hint: scope.toLowerCase() === "is" ? "filter" : "text extraction",
        apply: complete(`${scope.toLowerCase()}:${v}`),
        kind: "operator" as const,
      }));
  }

  // Completing an operator name?
  if (tail && !tail.includes(":") && /[a-z]/i.test(tail)) {
    const frag = tail.toLowerCase();
    const ops = OPERATORS.filter((o) => o.token.startsWith(frag)).map((o) => ({
      key: o.token,
      label: o.token,
      hint: o.hint,
      apply: complete(o.token),
      kind: "operator" as const,
    }));
    if (ops.length > 0) return ops;
  }

  // Default: recent searches, then popular operators.
  const items: SuggestItem[] = recents
    .filter((r) => !tail || r.toLowerCase().includes(tail.toLowerCase()))
    .slice(0, 5)
    .map((r) => ({ key: `recent:${r}`, label: r, hint: "recent", apply: r, kind: "recent" as const }));
  if (query.trim() === "") {
    for (const o of OPERATORS.slice(0, 4)) {
      items.push({ key: o.token, label: o.token, hint: o.hint, apply: o.token, kind: "operator" as const });
    }
  }
  return items.slice(0, 8);
}

export interface SuggestHandle {
  move(dir: 1 | -1): void;
  /** Apply the highlighted item; returns its kind, or null when empty. */
  choose(): SuggestItem["kind"] | null;
}

export type SuggestKind = SuggestItem["kind"];

interface Props {
  query: string;
  tags: TagInfo[];
  collections: CollectionInfo[];
  recents: string[];
  onClearRecents: () => void;
  onApply: (query: string, kind: SuggestKind) => void;
}

export const SearchSuggest = forwardRef<SuggestHandle, Props>(function SearchSuggest(
  { query, tags, collections, recents, onClearRecents, onApply },
  ref
) {
  const [active, setActive] = useState(0);
  const items = useMemo(
    () => computeSuggestions(query, tags, collections, recents),
    [query, tags, collections, recents]
  );

  useImperativeHandle(ref, () => ({
    move(dir: 1 | -1) {
      if (items.length === 0) return;
      setActive((a) => (a + dir + items.length) % items.length);
    },
    choose() {
      const item = items[Math.min(active, items.length - 1)];
      if (!item) return null;
      // Highlighting something that changes nothing counts as submit.
      if (item.apply.trim() === query.trim()) return null;
      onApply(item.apply, item.kind);
      setActive(0);
      return item.kind;
    },
  }), [items, active, onApply, query]);

  if (items.length === 0) return null;
  return (
    <ul className="suggest-menu" role="listbox" aria-label="Search suggestions">
      {items.map((item, i) => (
        <li key={item.key} role="presentation">
          <button
            role="option"
            aria-selected={i === active}
            className={`suggest-item${i === active ? " active" : ""}`}
            onMouseDown={(e) => {
              e.preventDefault(); // apply before input blur closes the menu
              onApply(item.apply, item.kind);
            }}
            onMouseEnter={() => setActive(i)}
          >
            <span className="suggest-ic" aria-hidden="true">
              {item.kind === "recent" ? <Icons.clock size={13} /> :
               item.kind === "tag" ? <Icons.tag size={13} /> :
               item.kind === "collection" ? <Icons.folder size={13} /> :
               <Icons.search size={13} />}
            </span>
            <span className="suggest-label">{item.label}</span>
            {item.hint && <span className="suggest-hint">{item.hint}</span>}
          </button>
        </li>
      ))}
      {recents.length > 0 && (
        <li role="presentation">
          <button
            className="suggest-clear"
            onMouseDown={(e) => {
              e.preventDefault();
              onClearRecents();
            }}
          >
            Clear recent searches
          </button>
        </li>
      )}
    </ul>
  );
});
