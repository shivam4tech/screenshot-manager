import { useEffect, useState } from "react";
import { api, type AppStateDto, type LibraryStats } from "../api";

/** Bottom status bar: index health at a glance. */
export default function StatusBar({
  appState,
  onRefresh,
}: {
  appState: AppStateDto | null;
  onRefresh: () => Promise<AppStateDto | null>;
}) {
  const [stats, setStats] = useState<LibraryStats | null>(null);

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
    </footer>
  );
}
