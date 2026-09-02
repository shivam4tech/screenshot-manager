import { useEffect, useState } from "react";
import {
  api,
  onScanComplete,
  onScanProgress,
  type ScanProgress,
  type ScanSummary,
} from "../api";

type Step = "welcome" | "folders" | "scanning" | "done";

/**
 * First-run experience: Welcome → Choose folders → Start indexing → Progress.
 * Designed to take under a minute. No accounts, no cloud, no concepts to learn.
 */
export default function Onboarding({ onFinish }: { onFinish: () => void }) {
  const [step, setStep] = useState<Step>("welcome");
  const [defaults, setDefaults] = useState<string[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [summary, setSummary] = useState<ScanSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.getDefaultDirectories().then((dirs) => {
      setDefaults(dirs);
      setSelected(new Set(dirs));
    });
  }, []);

  useEffect(() => {
    if (step !== "scanning") return;
    const un1 = onScanProgress((p) => setProgress(p));
    const un2 = onScanComplete((s) => {
      setSummary(s);
      setStep("done");
    });
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, [step]);

  const toggle = (p: string) => {
    const next = new Set(selected);
    if (next.has(p)) next.delete(p);
    else next.add(p);
    setSelected(next);
  };

  const addFolder = async () => {
    try {
      const picked = await api.pickFolder();
      if (picked) {
        setDefaults((d) => (d.includes(picked) ? d : [...d, picked]));
        setSelected((s) => new Set(s).add(picked));
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const start = async () => {
    try {
      for (const p of selected) {
        await api.addDirectory(p);
      }
      await api.startScan();
      setStep("scanning");
    } catch (e) {
      setError(String(e));
    }
  };

  if (step === "welcome") {
    return (
      <div className="center-screen onboarding">
        <div className="welcome-card">
          <h1>Remember everything you screenshot.</h1>
          <p className="lede">
            Search your screenshots by words, dates, applications, and tags.
          </p>
          <ul className="privacy-points">
            <li>Your screenshots stay on your computer.</li>
            <li>We don't move or upload them.</li>
            <li>Everything works offline.</li>
          </ul>
          <button className="primary" onClick={() => setStep("folders")}>
            Get started
          </button>
        </div>
      </div>
    );
  }

  if (step === "folders") {
    return (
      <div className="center-screen onboarding">
        <div className="welcome-card wide">
          <h2>Where are your screenshots?</h2>
          <p className="muted">
            We found the usual locations. Add any other folders you like — you
            can change this later in Settings.
          </p>
          <div className="folder-list">
            {defaults.map((d) => (
              <label key={d} className="folder-row">
                <input
                  type="checkbox"
                  checked={selected.has(d)}
                  onChange={() => toggle(d)}
                />
                <span className="mono">{d}</span>
              </label>
            ))}
          </div>
          <div className="row-gap">
            <button onClick={addFolder}>Add folder…</button>
            <button className="primary" disabled={selected.size === 0} onClick={start}>
              Start indexing
            </button>
          </div>
          {error && <p className="error">{error}</p>}
        </div>
      </div>
    );
  }

  if (step === "scanning") {
    const found = progress?.files_found ?? 0;
    const processed = progress?.files_processed ?? 0;
    const pct = found > 0 ? Math.round((processed / found) * 100) : 0;
    return (
      <div className="center-screen onboarding">
        <div className="welcome-card wide">
          <h2>Scanning your screenshots…</h2>
          <p className="big-count">{found} screenshots found</p>
          <div className="progress-track">
            <div className="progress-fill" style={{ width: `${pct}%` }} />
          </div>
          <div className="scan-stats">
            <div>
              <span className="muted">Indexed:</span> {progress?.files_indexed ?? 0}
            </div>
            <div>
              <span className="muted">Remaining:</span> {Math.max(0, found - processed)}
            </div>
            <div>
              <span className="muted">Problems:</span> {progress?.files_failed ?? 0}
            </div>
          </div>
          {progress?.current_file && (
            <p className="mono current-file">{progress.current_file}</p>
          )}
          <p className="muted small">
            Full-text OCR is applied in the background after indexing — your
            library is searchable by name and date right away.
          </p>
          <div className="row-gap">
            <button onClick={() => api.cancelScan()}>
              Pause (you can resume later)
            </button>
          </div>
        </div>
      </div>
    );
  }

  // done
  return (
    <div className="center-screen onboarding">
      <div className="welcome-card wide">
        <h2>Your screenshots are searchable.</h2>
        <p className="lede">
          {summary
            ? `${summary.indexed} indexed${
                summary.failed ? `, ${summary.failed} need attention` : ""
              }${summary.cancelled ? " (scan paused — resume anytime by re-running the scan)" : ""}.`
            : "Scan complete."}
        </p>
        <p className="muted">Try searching for things like:</p>
        <div className="chips">
          {["error", "research", "github", "invoice", "python"].map((w) => (
            <span key={w} className="chip">
              {w}
            </span>
          ))}
        </div>
        <button className="primary" onClick={onFinish}>
          Open my library
        </button>
      </div>
    </div>
  );
}
