import { useEffect, useState } from "react";
import { api, thumbnailUrl, type ScreenshotDetail } from "../api";

/** Minimal detail overlay: full thumbnail + metadata + OCR text. */
export default function Detail({ id, onClose }: { id: number; onClose: () => void }) {
  const [detail, setDetail] = useState<ScreenshotDetail | null>(null);
  const [imgUrl, setImgUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api
      .getScreenshot(id)
      .then(async (d) => {
        if (!alive || !d) return;
        setDetail(d);
        setImgUrl(await thumbnailUrl(d.content_hash, 1024));
      })
      .catch((e) => alive && setError(String(e)));
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      alive = false;
      window.removeEventListener("keydown", onKey);
    };
  }, [id, onClose]);

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
                {detail.tags.length > 0 && (
                  <>
                    <dt>Tags</dt>
                    <dd>{detail.tags.join(", ")}</dd>
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
