import { useCallback, useEffect, useState } from "react";
import { api, type ClassifySummary, type Problem } from "../api";
import { ACCENTS, type Accent, type ThemePref } from "../theme";
import { Icons } from "./icons";
import { Button, Toggle } from "./ui";

/**
 * Settings: appearance (accent color), OCR switch + enrichment, index
 * health (problems), and where local data lives. Watched folders are
 * managed from the sidebar.
 */
export default function Settings({
  accent,
  onAccentChange,
  customHex,
  onCustomAccent,
  themePref,
  onThemePrefChange,
}: {
  accent: Accent;
  onAccentChange: (a: Accent) => void;
  customHex: string;
  onCustomAccent: (hex: string) => void;
  themePref: ThemePref;
  onThemePrefChange: (p: ThemePref) => void;
}) {
  const [problems, setProblems] = useState<Problem[]>([]);
  const [ocrEnabled, setOcrEnabled] = useState(true);
  const [keepMemory, setKeepMemory] = useState(false);
  const [keepThumb, setKeepThumb] = useState(false);
  const [dataDir, setDataDir] = useState("");
  const [classifyNote, setClassifyNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copiedDir, setCopiedDir] = useState(false);

  const refresh = useCallback(async () => {
    const [p, ocr, dir, mem, thumb] = await Promise.all([
      api.listProblems(50),
      api.getSetting("ocr_enabled"),
      api.getDataDir(),
      api.getSetting("keep_deleted_memory"),
      api.getSetting("keep_deleted_thumbnail"),
    ]);
    setProblems(p);
    setOcrEnabled(ocr !== "0");
    setDataDir(dir);
    setKeepMemory(mem === "1");
    setKeepThumb(thumb === "1");
  }, []);

  useEffect(() => {
    refresh().catch((e) => setError(String(e)));
  }, [refresh]);

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

  const toggleSetting = async (
    key: "keep_deleted_memory" | "keep_deleted_thumbnail",
    next: boolean,
    apply: (v: boolean) => void,
    current: boolean
  ) => {
    apply(next);
    try {
      await api.setSetting(key, next ? "1" : "0");
    } catch (e) {
      setError(String(e));
      apply(current);
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

  const runClassification = async () => {    setBusy(true);
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
      <div className="page-head">
        <div>
          <h1 className="page-title">Settings</h1>
          <p className="page-sub">Configure how Screenshot Memory works</p>
        </div>
      </div>
      {error && <p className="error">{error}</p>}

      <section className="set-card">
        <span className="set-ic"><Icons.image size={18} /></span>
        <div className="set-body">
          <h3>Appearance</h3>
          <p className="muted small" style={{ margin: "0" }}>
            Accent color
          </p>
          <p className="muted small" style={{ margin: "2px 0 0" }}>
            Used for active and selected states. Status and error colors stay unchanged.
          </p>
          <div className="accent-grid" role="radiogroup" aria-label="Accent color">
            {ACCENTS.map((a) => (
              <button
                key={a.id}
                role="radio"
                aria-checked={accent === a.id}
                className={`accent-swatch${accent === a.id ? " on" : ""}`}
                onClick={() => onAccentChange(a.id)}
                title={`${a.label} accent`}
              >
                <span className="accent-dot" style={{ background: a.dot }} aria-hidden="true" />
                {a.label}
                {accent === a.id && <span className="accent-check"><Icons.check size={12} /></span>}
              </button>
            ))}
            <label
              className={`accent-swatch accent-custom${accent === "custom" ? " on" : ""}`}
              title="Custom accent color"
            >
              <input
                type="color"
                className="accent-color-input"
                aria-label="Custom accent color"
                value={/^#[0-9a-fA-F]{6}$/.test(customHex) ? customHex : "#4f6ef7"}
                onChange={(e) => onCustomAccent(e.target.value)}
              />
              Custom
              {accent === "custom" && <span className="accent-check"><Icons.check size={12} /></span>}
            </label>
          </div>
          {accent === "custom" && (
            <p className="muted small mono" style={{ margin: "8px 0 0" }}>{customHex}</p>
          )}
          <div className="accent-preview" aria-hidden="true">
            <span className="preview-nav">Selected item</span>
            <span className="preview-btn">Primary button</span>
          </div>
          <div className="theme-pref-row" role="radiogroup" aria-label="Appearance mode">
            <span className="muted small">Mode</span>
            {(["light", "system", "dark"] as const).map((m) => (
              <button
                key={m}
                role="radio"
                aria-checked={themePref === m}
                className={`theme-pref${themePref === m ? " on" : ""}`}
                onClick={() => onThemePrefChange(m)}
                title={m === "system" ? "Follow the operating system" : `${m[0].toUpperCase()}${m.slice(1)} mode`}
              >
                {m === "light" ? "Light" : m === "dark" ? "Dark" : "System"}
              </button>
            ))}
          </div>
        </div>
      </section>

      <section className="set-card">
        <span className="set-ic"><Icons.fileText size={18} /></span>
        <div className="set-body">
          <div className="set-row">
            <div className="grow">
              <h3>Text extraction (OCR)</h3>
              <p className="muted small" style={{ margin: "0" }}>
                Extract text from screenshots in the background using Tesseract.
                Fully local via the Tesseract sidecar — without the binary on
                PATH, extraction stays disabled and everything else keeps working.
              </p>
            </div>
            <Toggle checked={ocrEnabled} onChange={() => void toggleOcr()} label="Toggle OCR text extraction" />
          </div>
        </div>
      </section>

      <section className="set-card">
        <span className="set-ic"><Icons.tag size={18} /></span>
        <div className="set-body">
          <h3>Enrichment</h3>
          <p className="muted small" style={{ margin: "0 0 8px" }}>
            Guess source app, website, and category from filenames, paths, and
            extracted text. Runs automatically after scans — rerun anytime.
          </p>
          <div className="set-row">
            <Button size="sm" variant="primary" onClick={() => void runClassification()} disabled={busy}>
              {busy ? "Enriching…" : "Enrich now"}
            </Button>
            {classifyNote && (
              <span className="muted small" role="status">{classifyNote}</span>
            )}
          </div>
        </div>
      </section>

      <section className="set-card">
        <span className="set-ic"><Icons.drive size={18} /></span>
        <div className="set-body">
          <div className="set-row">
            <div className="grow">
              <h3>Keep searchable record after cleanup</h3>
              <p className="muted small" style={{ margin: "0" }}>
                Retain filename, metadata, tags, and OCR text of trashed
                screenshots. Metadata only — never a backup.
              </p>
            </div>
            <Toggle
              checked={keepMemory}
              onChange={() => void toggleSetting("keep_deleted_memory", !keepMemory, setKeepMemory, keepMemory)}
              label="Keep searchable record after cleanup"
            />
          </div>
          <div className="set-row" style={{ marginTop: 10 }}>
            <div className="grow">
              <h3>Keep tiny preview</h3>
              <p className="muted small" style={{ margin: "0" }}>
                Show the cached thumbnail on retained records when available.
              </p>
            </div>
            <Toggle
              checked={keepThumb}
              onChange={() => void toggleSetting("keep_deleted_thumbnail", !keepThumb, setKeepThumb, keepThumb)}
              label="Keep tiny preview on retained records"
            />
          </div>
        </div>
      </section>

      <section className="set-card">
        <span className="set-ic"><Icons.zap size={18} /></span>
        <div className="set-body">
          <h3>Index health</h3>
          {problems.length === 0 ? (
            <p className="muted small" style={{ margin: 0 }}>
              <span className="status-ok">●</span> No problems recorded.
            </p>
          ) : (
            <>
              <ul className="problem-list">
                {problems.map((p) => (
                  <li key={p.id}>
                    <span className="mono small">{p.path ?? "(unknown file)"}</span>
                    <span className="muted small"> [{p.kind}] {p.message}</span>
                  </li>
                ))}
              </ul>
              <Button size="sm" onClick={() => void clearProblems()}>Clear problems</Button>
            </>
          )}
        </div>
      </section>

      <section className="set-card">
        <span className="set-ic"><Icons.folder size={18} /></span>
        <div className="set-body">
          <h3>About</h3>
          <p className="muted small" style={{ margin: "0 0 8px" }}>
            Screenshot Memory 1.0 — local-first, offline, non-destructive.
            Watched folders are managed from the sidebar.
          </p>
          <div className="set-row">
            <div className="grow">
              <div className="insp-label">Data location</div>
              <div className="mono small muted path-trunc" title={dataDir || "(loading…)"}>
                {dataDir || "(loading…)"}
              </div>
            </div>
            <Button
              size="sm"
              variant="ghost"
              icon="copy"
              disabled={!dataDir}
              onClick={() => {
                void navigator.clipboard.writeText(dataDir).then(() => {
                  setCopiedDir(true);
                  setTimeout(() => setCopiedDir(false), 1600);
                });
              }}
              title="Copy data location"
            >
              {copiedDir ? "Copied" : "Copy"}
            </Button>
          </div>
        </div>
      </section>
    </div>
  );
}
