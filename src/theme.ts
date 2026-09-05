/** Explicit light/dark theme + user-pickable accent. Persisted. */

export type Theme = "light" | "dark";

const KEY = "shotmemory-theme";

export function initialTheme(): Theme {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved === "light" || saved === "dark") return saved;
  } catch {
    /* private mode etc. — fall through to OS preference */
  }
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

export function applyTheme(t: Theme) {
  document.documentElement.dataset.theme = t;
  try {
    localStorage.setItem(KEY, t);
  } catch {
    /* ignore */
  }
}

/**
 * Accent identity. Curated presets ship tuned light+dark values that keep
 * readable contrast for text-on-accent and accent-on-surface usage.
 * Semantic colors (danger/success/warning) stay fixed and separate.
 */
export type Accent =
  | "blue" | "cyan" | "teal" | "green" | "violet"
  | "purple" | "rose" | "red" | "orange" | "amber"
  | "custom";

const AKEY = "shotmemory-accent";
const CKEY = "shotmemory-accent-custom";

export const ACCENTS: Array<{ id: Exclude<Accent, "custom">; label: string; dot: string }> = [
  { id: "blue", label: "Blue", dot: "#4F6EF7" },
  { id: "cyan", label: "Cyan", dot: "#0891B2" },
  { id: "teal", label: "Teal", dot: "#0F9D8A" },
  { id: "green", label: "Green", dot: "#21A366" },
  { id: "violet", label: "Violet", dot: "#7357E8" },
  { id: "purple", label: "Purple", dot: "#8B4ED8" },
  { id: "rose", label: "Rose", dot: "#D94672" },
  { id: "red", label: "Red", dot: "#E5484D" },
  { id: "orange", label: "Orange", dot: "#E86F2C" },
  { id: "amber", label: "Amber", dot: "#D89214" },
];

export function initialAccent(): Accent {
  try {
    const saved = localStorage.getItem(AKEY);
    if (saved === "custom" || ACCENTS.some((a) => a.id === saved)) return saved as Accent;
  } catch {
    /* ignore */
  }
  return "blue";
}

export function getCustomHex(): string {
  try {
    const saved = localStorage.getItem(CKEY);
    if (saved && /^#[0-9a-fA-F]{6}$/.test(saved)) return saved;
  } catch {
    /* ignore */
  }
  return "#4F6EF7";
}

/* ---- small color utils (no dependencies) ---- */

function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function toHex(r: number, g: number, b: number): string {
  const c = (v: number) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0");
  return `#${c(r)}${c(g)}${c(b)}`;
}

function mix(hex: string, target: [number, number, number], t: number): string {
  const [r, g, b] = hexToRgb(hex);
  return toHex(r + (target[0] - r) * t, g + (target[1] - g) * t, b + (target[2] - b) * t);
}

function rgba(hex: string, a: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgb(${r} ${g} ${b} / ${a})`;
}

function luminance(hex: string): number {
  const f = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  const [r, g, b] = hexToRgb(hex).map(f);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [l1, l2] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (l1 + 0.05) / (l2 + 0.05);
}

/** Nudge a color toward black/white until it reaches min contrast vs bg. */
function ensureContrast(fg: string, bg: string, min: number): string {
  let c = fg;
  for (let i = 0; i < 24 && contrast(c, bg) < min; i++) {
    c = luminance(c) > luminance(bg) ? mix(c, [255, 255, 255], 0.08) : mix(c, [0, 0, 0], 0.08);
  }
  return c;
}

const INLINE_VARS = ["--accent", "--accent-hover", "--accent-active", "--accent-soft", "--accent-strong", "--ring"] as const;

export function clearCustomVars() {
  const el = document.documentElement;
  for (const v of INLINE_VARS) el.style.removeProperty(v);
}

/** Derive a full accent token set from one custom hex for the given theme. */
export function customVars(hex: string, theme: Theme): Record<string, string> {
  const base = theme === "light" ? ensureContrast(hex, "#ffffff", 3) : ensureContrast(hex, "#0b0f16", 3);
  if (theme === "light") {
    return {
      "--accent": base,
      "--accent-hover": mix(base, [0, 0, 0], 0.1),
      "--accent-active": mix(base, [0, 0, 0], 0.2),
      "--accent-soft": rgba(base, 0.09),
      "--accent-strong": mix(base, [0, 0, 0], 0.26),
      "--ring": rgba(base, 0.16),
    };
  }
  return {
    "--accent": base,
    "--accent-hover": mix(base, [255, 255, 255], 0.1),
    "--accent-active": mix(base, [0, 0, 0], 0.07),
    "--accent-soft": rgba(base, 0.14),
    "--accent-strong": mix(base, [255, 255, 255], 0.3),
    "--ring": rgba(base, 0.22),
  };
}

/**
 * Single entry point: curated presets via data-accent tokens, custom via
 * derived inline vars. Persists both choices.
 */
export function syncAccent(a: Accent, customHex: string, theme: Theme) {
  const el = document.documentElement;
  try {
    localStorage.setItem(AKEY, a);
    if (a === "custom") localStorage.setItem(CKEY, customHex);
  } catch {
    /* ignore */
  }
  if (a === "custom") {
    el.dataset.accent = "custom";
    const vars = customVars(customHex, theme);
    for (const [k, v] of Object.entries(vars)) el.style.setProperty(k, v);
  } else {
    clearCustomVars();
    el.dataset.accent = a;
  }
}
