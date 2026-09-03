import { useCallback, useEffect, useState } from "react";
import {
  api,
  type ClassifySummary,
  type DirectoryDto,
  type Problem,
} from "../api";

/**
 * Settings: watched folders, OCR switch + enrichment, index health
 * (problems), and where local data lives. The screen the empty state
 * already promised.
 */
export default function Settings({ onFoldersChanged }: { onFoldersChanged: () => void }) {
  const [dirs, setDirs] = useState<DirectoryDto[]>([]);
  const [problems, setProblems] = useState<Problem[]>([]);
  const [ocrEnabled, setOcrEnabled] = useState(true);
  const [dataDir, setDataDir] = useState("");
  const [classifyNote, setClassifyNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const [d, p, ocr, dir] = await Promise.all([
      api.listDirectories(),
      api.listProblems(50),
      api.getSetting("ocr_enabled"),
      api.getDataDir(),
    ]);
    setDirs(d);
    setProblems(p);
    setOcrEnabled(ocr !== "0");
    setDataDir(dir);
  }, []);

  useEffect(() => {
    refresh().catch((e) => setError(String(e)));
  }, [refresh]);

  const addFolder = async () => {
    setError(null);
    try {
      const picked = await api.pickFolder();
      if (!picked) return;
      await api.addDirectory(picked);
      await refresh();
      onFoldersChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const removeFolder = async (id: number) => {
    setError(null);
    try {
      await api.removeDirectory(id);
      await refresh();
      onFoldersChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const toggleOcr = async () => {
    const next = !ocrEnabled;
    setOcrEnabled(next);
    try {
      await api.setSetting("ocr_enabled", next ? "1" : "0");
    } catch (e) {
      setError(String(e));
      setOcrEnabled(!next);
    }
  };

  const clearProblems = async () => {
    try {
      await api.clearProblems();
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const runClassification = async () => {
    setBusy(true);
    setClassifyNote(null);
    setError(null);
    try {
      const s: ClassifySummary = await api.runClassification();
      setClassifyNote(
        `Enriched ${s.updated} of ${s.examined} screenshots (app, site, category).`
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings">
      <h2>Settings</h2>
      {error && <p className="error">{error}</p>}

      <section>
        <h3>Watched folders</h3>
        <p className="muted small">
          New screenshots here are indexed live. Removing a folder stops
          watching it — indexed records are kept.
        </p>
        <ul className="folder-list settings-folders">
          {dirs.map((d) => (
            <li key={d.id} className="folder-row">
              <span className="mono">{d.path}</span>
              <button
                className="icon-btn"
                aria-label={`Stop watching ${d.path}`}
                title={`Stop watching ${d.path}`}
                onClick={() => removeFolder(d.id)}
              >
                ✕
              </button>
            </li>
          ))}
        </ul>
        <button onClick={addFolder}>Add folder…</button>
      </section>

      <section>
        <h3>Text extraction (OCR)</h3>
        <label className="check-row">
          <input type="checkbox" checked={ocrEnabled} onChange={toggleOcr} />
          Extract text from screenshots in the background
        </label>
        <p className="muted small">
          Fully local via the Tesseract sidecar. Needs the{" "}
          <span className="mono">tesseract</span> binary on PATH — without it,
          extraction stays disabled and everything else keeps working.
        </p>
      </section>

      <section>
        <h3>Enrichment</h3>
        <p className="muted small">
          Guess source app, website, and category from filenames, paths, and
          extracted text. Runs automatically after scans — rerun anytime.
        </p>
        <button onClick={runClassification} disabled={busy}>
          {busy ? "Enriching…" : "Enrich now"}
        </button>
        {classifyNote && (
          <p className="muted small" role="status">
            {classifyNote}
          </p>
        )}
      </section>

      <section>
        <h3>Index health</h3>
        {problems.length === 0 ? (
          <p className="muted small">No problems recorded.</p>
        ) : (
          <>
            <ul className="problem-list">
              {problems.map((p) => (
                <li key={p.id}>
                  <span className="mono small">{p.path ?? "(unknown file)"}</span>
                  <span className="muted small">
                    {" "}
                    [{p.kind}] {p.message}
                  </span>
                </li>
              ))}
            </ul>
            <button onClick={clearProblems}>Clear problems</button>
          </>
        )}
      </section>

      <section>
        <h3>About</h3>
        <p className="muted small">
          Screenshot Memory 1.0 — local-first, offline, non-destructive.
        </p>
        <p className="muted small">
          Data lives at <span className="mono">{dataDir || "(loading…)"}</span>
        </p>
      </section>
    </div>
  );
}
