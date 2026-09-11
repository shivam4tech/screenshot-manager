import { useCallback, useEffect, useState } from "react";
import { api, type RenameEntry, type RenamePlan } from "../api";
import { Button, Dropdown } from "./ui";

const PATTERN_KEY = "shotmemory-rename-patterns";
const PATTERN_MAX = 6;

function loadPatterns(): string[] {
  try {
    const raw = localStorage.getItem(PATTERN_KEY);
    const arr = raw ? (JSON.parse(raw) as unknown) : [];
    return Array.isArray(arr) ? arr.filter((s): s is string => typeof s === "string").slice(0, PATTERN_MAX) : [];
  } catch {
    return [];
  }
}

function savePattern(pattern: string) {
  const p = pattern.trim();
  if (!p) return;
  try {
    const next = [p, ...loadPatterns().filter((s) => s !== p)].slice(0, PATTERN_MAX);
    localStorage.setItem(PATTERN_KEY, JSON.stringify(next));
  } catch {
    /* ignore */
  }
}

const VARS: Array<[string, string]> = [
  ["{counter}", "sequence number"],
  ["{date}", "capture date"],
  ["{time}", "capture time"],
  ["{original}", "original name"],
  ["{collection}", "first collection"],
  ["{source}", "source app"],
];

export interface AppliedRename {
  id: number;
  old_path: string;
  new_path: string;
}

/**
 * Bulk rename dialog. Everything is preview-only until Apply: the plan is
 * computed server-side (template + sanitizing + collision detection) and
 * Apply executes explicit targets, so what you see is what runs.
 */
export default function RenameDialog({
  ids,
  onClose,
  onDone,
}: {
  ids: number[];
  onClose: () => void;
  onDone: (applied: AppliedRename[]) => void;
}) {
  const [pattern, setPattern] = useState("screenshot_{counter}");
  const [counterStart, setCounterStart] = useState(1);
  const [padding, setPadding] = useState(3);
  const [order, setOrder] = useState("grid");
  const [strategy, setStrategy] = useState("stop");
  const [plan, setPlan] = useState<RenamePlan | null>(null);
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [patterns, setPatterns] = useState<string[]>(() => loadPatterns());

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const p = await api.renamePreview(ids, {
        pattern,
        counter_start: Math.max(0, Math.floor(counterStart) || 0),
        padding: Math.min(6, Math.max(1, Math.floor(padding) || 3)),
        order,
        strategy,
      });
      setPlan(p);
    } catch (e) {
      setPlan(null);
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [ids, pattern, counterStart, padding, order, strategy]);

  useEffect(() => {
    const t = setTimeout(() => void refresh(), 300);
    return () => clearTimeout(t);
  }, [refresh]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const okEntries: RenameEntry[] = (plan?.entries ?? []).filter((e) => e.status === "ok");
  const badEntries = (plan?.entries ?? []).filter((e) => e.status !== "ok");
  const shown = (plan?.entries ?? []).slice(0, 10);
  const hidden = (plan?.entries ?? []).length - shown.length;
  const canApply = !applying && !loading && okEntries.length > 0 &&
    (strategy === "append" || badEntries.filter((e) => e.status === "conflict").length === 0);

  const apply = async () => {
    setApplying(true);
    setError(null);
    try {
      const out = await api.renameExecute(
        okEntries.map((e) => ({ id: e.id, new_path: e.new_path }))
      );
      savePattern(pattern);
      setPatterns(loadPatterns());
      const applied = out.results
        .filter((r) => r.ok && r.message !== "unchanged")
        .map((r) => ({ id: r.id, old_path: r.old_path, new_path: r.new_path }));
      if (out.failed > 0 || out.rolled_back > 0) {
        const bits = [`renamed ${out.renamed}`];
        if (out.skipped > 0) bits.push(`${out.skipped} skipped`);
        if (out.failed > 0) bits.push(`${out.failed} failed`);
        if (out.rolled_back > 0) bits.push(`${out.rolled_back} reversed`);
        const detail = out.results
          .filter((r) => !r.ok && r.id >= 0 && r.message)
          .slice(0, 5)
          .map((r) => `${r.new_path.split("/").pop()}: ${r.message}`)
          .join("\n");
        setError(`${bits.join(", ")}.\n${detail}`);
      }
      if (applied.length > 0) onDone(applied);
      else if (out.failed === 0) onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setApplying(false);
    }
  };

  const baseName = (p: string) => p.split("/").pop() ?? p;

  return (
    <div className="detail-backdrop" onClick={onClose} role="dialog" aria-modal="true" aria-label="Bulk rename">
      <div className="rename-panel" onClick={(e) => e.stopPropagation()}>
        <div className="detail-head">
          <h3>Rename {ids.length} screenshot{ids.length === 1 ? "" : "s"}</h3>
          <button className="iconbtn" onClick={onClose} aria-label="Close rename dialog">✕</button>
        </div>
        <label className="insp-label" htmlFor="rename-pattern">Naming pattern</label>
        <input
          id="rename-pattern"
          className="rename-pattern"
          type="text"
          value={pattern}
          onChange={(e) => setPattern(e.target.value)}
          placeholder="project_{counter}"
          aria-label="Naming pattern"
          spellCheck={false}
        />
        <div className="var-chips" aria-label="Template variables">
          {VARS.map(([v, hint]) => (
            <button
              key={v}
              className="var-chip"
              title={hint}
              onClick={() => setPattern((p) => (p ? `${p}_${v}` : v))}
            >
              {v}
            </button>
          ))}
        </div>
        {patterns.length > 0 && (
          <div className="var-chips" aria-label="Recent patterns">
            <span className="muted small">Recent:</span>
            {patterns.map((p) => (
              <button key={p} className="var-chip recent" onClick={() => setPattern(p)} title={`Use ${p}`}>
                {p}
              </button>
            ))}
          </div>
        )}
        <div className="rename-opts">
          <label>
            Starting number
            <input
              type="number"
              min={0}
              value={counterStart}
              onChange={(e) => setCounterStart(Number(e.target.value))}
              aria-label="Starting number"
            />
          </label>
          <label>
            Padding
            <input
              type="number"
              min={1}
              max={6}
              value={padding}
              onChange={(e) => setPadding(Number(e.target.value))}
              aria-label="Number padding"
            />
          </label>
          <Dropdown
            ariaLabel="Counter order"
            prefix="Order:"
            value={order}
            onChange={setOrder}
            options={[
              { value: "grid", label: "Selected order" },
              { value: "oldest", label: "Oldest first" },
              { value: "newest", label: "Newest first" },
              { value: "name", label: "Filename A–Z" },
            ]}
          />
          <Dropdown
            ariaLabel="On conflict"
            prefix="On conflict:"
            value={strategy}
            onChange={setStrategy}
            options={[
              { value: "stop", label: "Stop and ask" },
              { value: "append", label: "Append number" },
            ]}
          />
        </div>
        {plan && plan.dirs.length > 1 && (
          <p className="muted small">
            {plan.entries.length} screenshots across {plan.dirs.length} folders — each file stays in its folder.
          </p>
        )}
        <span className="insp-label">Preview</span>
        {loading && !plan ? (
          <p className="muted">Computing preview…</p>
        ) : plan && plan.entries.length > 0 ? (
          <>
            <ul className="rename-list">
              {shown.map((e) => (
                <li key={e.id} className={`rename-row ${e.status}`}>
                  <span className="rename-old" title={e.old_path}>{baseName(e.old_path)}</span>
                  <span className="rename-arrow" aria-hidden="true">→</span>
                  <span className="rename-new" title={e.status === "ok" ? e.new_path : e.message || e.new_path}>
                    {e.status === "ok" ? baseName(e.new_path) : (e.message || baseName(e.new_path))}
                  </span>
                  {e.sanitized && e.status === "ok" && <span className="rename-flag" title="Adjusted for cross-platform safety">adjusted</span>}
                  {e.status === "conflict" && <span className="rename-flag bad">conflict</span>}
                  {e.status === "skipped" && <span className="rename-flag">skipped</span>}
                </li>
              ))}
            </ul>
            {hidden > 0 && <p className="muted small">+ {hidden} more</p>}
          </>
        ) : (
          !error && <p className="muted small">Type a pattern to preview the new names.</p>
        )}
        {error && <p className="error small preview-error">{error}</p>}
        <div className="confirm-actions">
          <Button size="sm" variant="ghost" onClick={onClose} disabled={applying}>
            Cancel
          </Button>
          <Button size="sm" variant="primary" onClick={() => void apply()} disabled={!canApply}>
            {applying ? "Renaming…" : `Rename ${okEntries.length} screenshot${okEntries.length === 1 ? "" : "s"}`}
          </Button>
        </div>
      </div>
    </div>
  );
}
