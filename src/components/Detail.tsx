import { useCallback, useEffect, useState } from "react";
import {
  api,
  thumbnailUrl,
  type CollectionInfo,
  type ScreenshotDetail,
} from "../api";

/**
 * Detail overlay: full thumbnail + metadata + OCR text, plus Sprint 3
 * organization editing — star, read-later, note, tags, collections.
 * Calls `onChanged` after any mutation so the library grid/sidebar refresh.
 */
export default function Detail({
  id,
  onClose,
  onChanged,
}: {
  id: number;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [detail, setDetail] = useState<ScreenshotDetail | null>(null);
  const [imgUrl, setImgUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [memberOf, setMemberOf] = useState<CollectionInfo[]>([]);
  const [allCollections, setAllCollections] = useState<CollectionInfo[]>([]);
  const [tagInput, setTagInput] = useState("");
  const [noteDraft, setNoteDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

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
    reload().catch((e) => alive && setError(String(e)));
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      alive = false;
      window.removeEventListener("keydown", onKey);
    };
  }, [id, onClose, reload]);

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

  const memberIds = new Set(memberOf.map((c) => c.id));
  const addable = allCollections.filter((c) => !memberIds.has(c.id));

  return (
    <div className="detail-backdrop" onClick={onClose} role="dialog" aria-modal="true">
      <div className="detail-panel" onClick={(e) => e.stopPropagation()}>
        <div className="detail-media">
          {imgUrl ? (
            <img src={imgUrl} alt={detail?.filename ?? ""} />
          ) : (
            <div className="thumb-placeholder" aria-hidden="true" />
          )}
        </div>
        <div className="detail-info">
          <div className="detail-head">
            <h3 title={detail?.filename}>{detail?.filename ?? "Loading…"}</h3>
            <button onClick={onClose} aria-label="Close">
              ✕
            </button>
          </div>
          {error && <p className="error">{error}</p>}
          {detail && (
            <>
              <p className="muted small">{dateLabel(detail.created_ts)}</p>
              <div className="detail-actions">
                <button
                  className={detail.starred ? "toggle-on" : ""}
                  disabled={saving}
                  onClick={() => mutate(() => api.setStarred(id, !detail.starred))}
                  aria-pressed={detail.starred}
                  title="Star (find via is:starred)"
                >
                  {detail.starred ? "★ Starred" : "☆ Star"}
                </button>
                <button
                  className={detail.read_later ? "toggle-on" : ""}
                  disabled={saving}
                  onClick={() =>
                    mutate(() => api.setReadLater(id, !detail.read_later))
                  }
                  aria-pressed={detail.read_later}
                  title="Save for later"
                >
                  {detail.read_later ? "◉ Read later" : "○ Read later"}
                </button>
              </div>
              <dl className="detail-meta">
                <dt>Dimensions</dt>
                <dd>
                  {detail.width && detail.height
                    ? `${detail.width} × ${detail.height} px`
                    : "—"}
                </dd>
                <dt>Format</dt>
                <dd>{detail.format ?? "—"}</dd>
                <dt>Status</dt>
                <dd>{detail.status}</dd>
                <dt>Path</dt>
                <dd className="mono small">{detail.path}</dd>
                {(detail.app_name || detail.category) && (
                  <>
                    <dt>Source</dt>
                    <dd>
                      {[detail.app_name, detail.website_domain, detail.category]
                        .filter(Boolean)
                        .join(" · ")}
                    </dd>
                  </>
                )}
                {detail.url && (
                  <>
                    <dt>Link</dt>
                    <dd className="mono small">{detail.url}</dd>
                  </>
                )}
                <dt>Tags</dt>
                <dd>
                  <div className="tag-row">
                    {detail.tags.map((t) => (
                      <span className="tag-chip" key={t}>
                        {t}
                        <button
                          className="tag-x"
                          disabled={saving}
                          onClick={() => mutate(() => api.removeTag(id, t))}
                          aria-label={`Remove tag ${t}`}
                        >
                          ✕
                        </button>
                      </span>
                    ))}
                    <span className="tag-add">
                      <input
                        type="text"
                        placeholder="add tag…"
                        aria-label="Add tag"
                        value={tagInput}
                        disabled={saving}
                        onChange={(e) => setTagInput(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") addTag();
                        }}
                      />
                    </span>
                  </div>
                </dd>
                <dt>Note</dt>
                <dd>
                  {noteDraft === null ? (
                    <div className="note-view">
                      {detail.note ? (
                        <p className="note-text">{detail.note}</p>
                      ) : (
                        <span className="muted">no note — searchable once added</span>
                      )}{" "}
                      <button
                        className="link-btn"
                        disabled={saving}
                        onClick={() => setNoteDraft(detail.note)}
                      >
                        {detail.note ? "Edit" : "Add"}
                      </button>
                    </div>
                  ) : (
                    <div className="note-edit">
                      <textarea
                        value={noteDraft}
                        disabled={saving}
                        onChange={(e) => setNoteDraft(e.target.value)}
                        rows={3}
                        aria-label="Note"
                      />
                      <div className="row-gap tight">
                        <button
                          className="primary"
                          disabled={saving}
                          onClick={saveNote}
                        >
                          Save
                        </button>
                        <button disabled={saving} onClick={() => setNoteDraft(null)}>
                          Cancel
                        </button>
                      </div>
                    </div>
                  )}
                </dd>
                <dt>In collections</dt>
                <dd>
                  {memberOf.length === 0 && addable.length === 0 ? (
                    <span className="muted">none yet</span>
                  ) : (
                    <div className="tag-row">
                      {memberOf.map((c) => (
                        <span className="tag-chip" key={c.id}>
                          {c.name}
                          <button
                            className="tag-x"
                            disabled={saving}
                            onClick={() =>
                              mutate(() => api.removeFromCollection(c.id, id))
                            }
                            aria-label={`Remove from ${c.name}`}
                          >
                            ✕
                          </button>
                        </span>
                      ))}
                    </div>
                  )}
                  {addable.length > 0 && (
                    <select
                      className="collection-add"
                      disabled={saving}
                      defaultValue=""
                      aria-label="Add to collection"
                      onChange={(e) => {
                        const cid = Number(e.target.value);
                        if (cid) void mutate(() => api.addToCollection(cid, id));
                        e.target.value = "";
                      }}
                    >
                      <option value="">+ add to collection…</option>
                      {addable.map((c) => (
                        <option key={c.id} value={c.id}>
                          {c.name}
                        </option>
                      ))}
                    </select>
                  )}
                </dd>
                {detail.ocr_status === "done" && (
                  <>
                    <dt>OCR text</dt>
                    <dd>
                      {detail.ocr_text && detail.ocr_text.trim() ? (
                        <pre className="ocr-text">{detail.ocr_text}</pre>
                      ) : (
                        <span className="muted">no text detected</span>
                      )}
                    </dd>
                  </>
                )}
                {detail.ocr_status === "failed" && (
                  <>
                    <dt>OCR</dt>
                    <dd className="error">failed — retry from the status bar</dd>
                  </>
                )}
              </dl>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
