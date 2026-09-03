/** Explicit light/dark theme. Persisted; defaults to the OS preference. */

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
