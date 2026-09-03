import { useCallback, useEffect, useState } from "react";
import {
  api,
  onOcrProgress,
  onOcrComplete,
  type AppStateDto,
  type LibraryStats,
  type OcrProgress,
} from "../api";

/**
 * Bottom status bar: index health + OCR pipeline state at a glance.
 * OCR progress arrives via `ocr://progress` / `ocr://complete` events; the
 * queue counts come from periodic stats polling.
 */
export default function StatusBar({
  appState,
  onRefresh,
}: {
  appState: AppStateDto | null;
  onRefresh: () => Promise<AppStateDto | null>;
}) {
  const [stats, setStats] = useState<LibraryStats | null>(null);
  const [ocr, setOcr] = useState<OcrProgress | null>(null);
  const [ocrNote, setOcrNote] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const tick = async () => {
      const st = await api.getStats().catch(() => null);
      if (alive && st) setStats(st);
    };
    tick();
    // While a scan runs, poll lightly so the count climbs live.
    const id = setInterval(() => {
      tick();
      if (appState?.indexing) onRefresh();
    }, 2000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [appState?.indexing, onRefresh]);

  useEffect(() => {
    let alive = true;
    const unsubs: Array<() => void> = [];
    onOcrProgress((p) => alive && setOcr(p.done ? null : p)).then((u) =>
      unsubs.push(u)
    );
    onOcrComplete((s) => {
      if (!alive) return;
      setOcr(null);
      setOcrNote(
        s.cancelled
          ? "OCR stopped"
          : s.failed > 0
            ? `OCR finished: ${s.succeeded} ok, ${s.failed} failed`
            : null
      );
      api.getStats().then((st) => alive && setStats(st)).catch(() => {});
    }).then((u) => unsubs.push(u));
    return () => {
      alive = false;
      unsubs.forEach((u) => u());
    };
  }, []);

  const startOcr = useCallback(() => {
    setOcrNote(null);
    api.startOcr().catch((e) => setOcrNote(String(e)));
  }, []);
  const cancelOcr = useCallback(() => api.cancelOcr().catch(() => {}), []);
  const retryOcr = useCallback(() => {
    setOcrNote(null);
    api
      .retryOcr()
      .then(() => api.getStats().then((st) => setStats(st)).catch(() => {}))
      .catch((e) => setOcrNote(String(e)));
  }, []);

  if (!stats) return <footer className="status-bar" />;

  const attention =
    stats.problem_count > 0 ? ` • ${stats.problem_count} need attention` : "";

  return (
    <footer className="status-bar">
      {appState?.indexing ? (
        <span>
          <span className="dot dot-active" aria-hidden="true" /> Indexing…{" "}
          {stats.total.toLocaleString()} indexed
        </span>
      ) : (
        <span>
          <span className="dot" aria-hidden="true" />{" "}
          {stats.total.toLocaleString()} screenshots indexed{attention}
        </span>
      )}

      <span className="status-ocr">
        {ocr ? (
          <>
            <span className="dot dot-active" aria-hidden="true" /> Reading text{" "}
            {ocr.processed}/{ocr.total}
            <button className="link-btn" onClick={cancelOcr}>
              Stop
            </button>
          </>
        ) : stats.ocr_failed > 0 ? (
          <>
            <span className="error">{stats.ocr_failed} failed OCR</span>
            <button className="link-btn" onClick={retryOcr}>
              Retry
            </button>
          </>
        ) : stats.ocr_pending > 0 ? (
          <>
            <span className="muted">{stats.ocr_pending} awaiting text extraction</span>
            <button className="link-btn" onClick={startOcr}>
              Start
            </button>
          </>
        ) : (
          ocrNote && <span className="muted">{ocrNote}</span>
        )}
      </span>
    </footer>
  );
}
