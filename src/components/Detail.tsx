import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  thumbnailUrl,
  type CollectionInfo,
  type ScreenshotDetail,
} from "../api";
import { Icons } from "./icons";
import { Button, Dropdown, IconButton } from "./ui";

/**
 * Detail overlay: large preview + metadata inspector, plus organization
 * editing — star, read-later, note, tags, collections.
 * Calls `onChanged` after any mutation so the library grid/sidebar refresh.
 */
export default function Detail({
  id,
  onClose,
  onChanged,
  onPrev,
  onNext,
  position,
}: {
  id: number;
  onClose: () => void;
  onChanged: () => void;
  onPrev?: () => void;
  onNext?: () => void;
  position?: string;
}) {
  const [detail, setDetail] = useState<ScreenshotDetail | null>(null);
  const [imgUrl, setImgUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [memberOf, setMemberOf] = useState<CollectionInfo[]>([]);
  const [allCollections, setAllCollections] = useState<CollectionInfo[]>([]);
  const [tagInput, setTagInput] = useState("");
  const [noteDraft, setNoteDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [copied, setCopied] = useState(false);
  const [copiedPath, setCopiedPath] = useState(false);
  const [tagEditing, setTagEditing] = useState(false);
  const lastZoom = useRef(2);

  const toggleFit = useCallback(() => {
    if (zoom === 1) {
      setZoom(lastZoom.current);
    } else {
      lastZoom.current = zoom;
      setZoom(1);
    }
  }, [zoom]);

  const reload = useCallback(async () => {
    const d = await api.getScreenshot(id);
    if (!d) return;
    setDetail(d);
    setImgUrl(await thumbnailUrl(d.content_hash, 1024));
    setMemberOf(await api.listScreenshotCollections(id));
    setAllCollections(await api.listCollections());
  }, [id]);

  useEffect(() => {
    let alive = true;
    setZoom(1);
    setCopied(false);
    reload().catch((e) => alive && setError(String(e)));
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const typing = t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT");
      if (e.key === "Escape") onClose();
      else if (!typing && e.key === "ArrowLeft") onPrev?.();
      else if (!typing && e.key === "ArrowRight") onNext?.();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      alive = false;
      window.removeEventListener("keydown", onKey);
    };
  }, [id, onClose, onPrev, onNext, reload]);

  const mutate = async (fn: () => Promise<unknown>) => {
    setSaving(true);
    setError(null);
    try {
      await fn();
      await reload();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const dateLabel = (ts: number | null) =>
    ts
      ? new Date(ts * 1000).toLocaleString(undefined, {
          year: "numeric",
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        })
      : "unknown";

  const addTag = () => {
    const name = tagInput.trim();
    if (!name) return;
    setTagInput("");
    void mutate(() => api.addTag(id, name));
  };

  const saveNote = () => {
    if (noteDraft === null) return;
    const text = noteDraft;
    setNoteDraft(null);
    void mutate(() => api.setNote(id, text));
  };

  const deleteSelf = () => {
    if (
      !window.confirm(
        "Move this screenshot to the trash?\n\nThe file goes to the OS trash (recoverable); its record stays in the library as missing."
      )
    )
      return;
    setSaving(true);
    api
      .deleteScreenshots([id])
      .then(() => {
        onChanged();
        onClose();
      })
      .catch((e) => {
        setError(String(e));
        setSaving(false);
      });
  };

  const copyOcr = async () => {
    if (!detail?.ocr_text) return;
    try {
      await navigator.clipboard.writeText(detail.ocr_text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch (e) {
      setError(String(e));
    }
  };

  const copyPath = async () => {
    if (!detail?.path) return;
    try {
      await navigator.clipboard.writeText(detail.path);
      setCopiedPath(true);
      setTimeout(() => setCopiedPath(false), 1600);
    } catch (e) {
      setError(String(e));
    }
  };

  const memberIds = new Set(memberOf.map((c) => c.id));
  const addable = allCollections.filter((c) => !memberIds.has(c.id));
  const source = detail ? [detail.app_name, detail.website_domain, detail.category].filter(Boolean).join(" · ") : "";

  return (
    <div className="detail-backdrop" onClick={onClose} role="dialog" aria-modal="true" aria-label="Screenshot detail">
      <div className="detail-panel" onClick={(e) => e.stopPropagation()}>
        <div className={`detail-media${zoom > 1 ? " zoomed" : ""}`}>
          {imgUrl ? (
            <img
              src={imgUrl}
              alt={detail?.filename ?? ""}
              style={zoom === 1 ? undefined : { transform: `scale(${zoom})`, transformOrigin: "center center" }}
              onDoubleClick={toggleFit}
              title="Double-click to toggle fit / actual size"
            />
          ) : (
            <div className="thumb-placeholder" aria-hidden="true" />
          )}
          {(onPrev || onNext) && (
            <>
              <button className="media-nav media-prev" onClick={onPrev} disabled={!onPrev} aria-label="Previous screenshot">‹</button>
              <button className="media-nav media-next" onClick={onNext} disabled={!onNext} aria-label="Next screenshot">›</button>
            </>
          )}
          <div className="media-zoombar" role="toolbar" aria-label="Preview controls">
            <button onClick={() => setZoom((z) => Math.max(0.5, +(z - 0.25).toFixed(2)))} disabled={zoom <= 0.5} aria-label="Zoom out">−</button>
            <button onClick={() => setZoom(1)} title="Reset to fit" aria-label="Reset zoom">{Math.round(zoom * 100)}%</button>
            <button onClick={() => setZoom((z) => Math.min(3, +(z + 0.25).toFixed(2)))} disabled={zoom >= 3} aria-label="Zoom in">+</button>
            <button
              onClick={toggleFit}
              title={zoom === 1 ? "Zoom to previous size" : "Fit to view"}
              aria-label={zoom === 1 ? "Zoom to previous size" : "Fit to view"}
              aria-pressed={zoom !== 1}
            >
              <Icons.expand size={13} />
            </button>
          </div>
        </div>
        <div className="detail-info">
          <div className="detail-head">
            <div style={{ minWidth: 0 }}>
              <h3 title={detail?.filename}>{detail?.filename ?? "Loading…"}</h3>
              <p className="detail-date">{detail ? dateLabel(detail.created_ts) : ""}{position ? ` · ${position}` : ""}</p>
            </div>
            <IconButton icon="x" label="Close" onClick={onClose} />
          </div>
          {error && <p className="error small">{error}</p>}
          {detail && (
            <>
              <div className="detail-actions">
                <Button
                  size="sm"
                  variant={detail.starred ? "secondary" : "ghost"}
                  aria-pressed={detail.starred}
                  disabled={saving}
                  onClick={() => mutate(() => api.setStarred(id, !detail.starred))}
                  title="Star (find via is:starred)"
                  icon="star"
                >
                  {detail.starred ? "Starred" : "Star"}
                </Button>
                <Button
                  size="sm"
                  variant={detail.read_later ? "secondary" : "ghost"}
                  aria-pressed={detail.read_later}
                  disabled={saving}
                  onClick={() => mutate(() => api.setReadLater(id, !detail.read_later))}
                  title="Save for later"
                  icon="bookmark"
                >
                  Read later
                </Button>
                <span className="spacer" />
                <Button size="sm" variant="danger" disabled={saving} onClick={deleteSelf} title="Move file to trash (record kept as missing)" icon="trash">
                  Delete
                </Button>
              </div>

              <dl className="kv">
                <dt>Dimensions</dt>
                <dd>{detail.width && detail.height ? `${detail.width} × ${detail.height} px` : "—"}</dd>
                <dt>Format</dt>
                <dd>{detail.format ?? "—"}</dd>
                <dt>Status</dt>
                <dd>{detail.status === "available" ? <span className="status-ok">● Available</span> : detail.status}</dd>
                <dt>Path</dt>
                <dd>
                  <span className="path-row">
                    <span className="mono path-trunc" title={detail.path}>{detail.path}</span>
                    <button
                      className="path-copy"
                      onClick={() => void copyPath()}
                      title="Copy full path"
                      aria-label="Copy full path"
                    >
                      {copiedPath ? <Icons.check size={12} /> : <Icons.copy size={12} />}
                    </button>
                  </span>
                </dd>
                {source && (<><dt>Source</dt><dd>{source}</dd></>)}
                {detail.url && (<><dt>Link</dt><dd className="mono">{detail.url}</dd></>)}
              </dl>

              <div className="insp-section">
                <span className="insp-label">Tags</span>
                <div className="tag-row">
                  {detail.tags.map((t) => (
                    <span className="tag-chip" key={t}>
                      {t}
                      <button className="tag-x" disabled={saving} onClick={() => mutate(() => api.removeTag(id, t))} aria-label={`Remove tag ${t}`}>✕</button>
                    </span>
                  ))}
                  {tagEditing ? (
                    <input
                      className="tag-inline-input"
                      type="text"
                      autoFocus
                      placeholder="Tag name…"
                      aria-label="Add tag"
                      value={tagInput}
                      disabled={saving}
                      onChange={(e) => setTagInput(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") { addTag(); setTagEditing(false); }
                        if (e.key === "Escape") { setTagEditing(false); setTagInput(""); }
                      }}
                      onBlur={() => { if (!tagInput.trim()) setTagEditing(false); }}
                    />
                  ) : (
                    <Button size="sm" variant="ghost" icon="plus" disabled={saving} onClick={() => setTagEditing(true)}>
                      Add tag
                    </Button>
                  )}
                </div>
              </div>

              <div className="insp-section">
                <span className="insp-label">Collections</span>
                <div className="tag-row">
                  {memberOf.map((c) => (
                    <span className="tag-chip" key={c.id}>
                      {c.name}
                      <button className="tag-x" disabled={saving} onClick={() => mutate(() => api.removeFromCollection(c.id, id))} aria-label={`Remove from ${c.name}`}>✕</button>
                    </span>
                  ))}
                </div>
                {addable.length > 0 && (
                  <Dropdown
                    value=""
                    active={false}
                    ariaLabel="Add to collection"
                    placeholder="+ Add to collection…"
                    options={addable.map((c) => ({ value: String(c.id), label: c.name }))}
                    onChange={(v) => {
                      const cid = Number(v);
                      if (cid) void mutate(() => api.addToCollection(cid, id));
                    }}
                  />
                )}
              </div>

              <div className="insp-section">
                <span className="insp-label">Note</span>
                {noteDraft === null ? (
                  <div>
                    {detail.note ? (
                      <>
                        <p className="note-text">{detail.note}</p>
                        <button className="link-btn" disabled={saving} onClick={() => setNoteDraft(detail.note)}>
                          Edit
                        </button>
                      </>
                    ) : (
                      <Button size="sm" variant="ghost" icon="plus" disabled={saving} onClick={() => setNoteDraft("")}>
                        Add note
                      </Button>
                    )}
                  </div>
                ) : (
                  <div className="note-edit">
                    <textarea value={noteDraft} disabled={saving} onChange={(e) => setNoteDraft(e.target.value)} rows={3} aria-label="Note" />
                    <div className="row-gap tight">
                      <button className="primary" disabled={saving} onClick={saveNote}>Save</button>
                      <button className="btn btn-md" disabled={saving} onClick={() => setNoteDraft(null)}>Cancel</button>
                    </div>
                  </div>
                )}
              </div>

              <div className="insp-section">
                <span className="insp-label" style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                  OCR text
                  {detail.ocr_text?.trim() && (
                    <button className="link-btn" onClick={() => void copyOcr()} title="Copy OCR text">
                      {copied ? "Copied ✓" : "⧉ Copy"}
                    </button>
                  )}
                </span>
                {detail.ocr_status === "done" ? (
                  detail.ocr_text && detail.ocr_text.trim() ? (
                    <pre className="ocr-box">{detail.ocr_text}</pre>
                  ) : (
                    <span className="muted small">No text detected</span>
                  )
                ) : detail.ocr_status === "failed" ? (
                  <span className="error small">Extraction failed — retry from the status bar</span>
                ) : (
                  <span className="muted small">Text extraction {detail.ocr_status}…</span>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
