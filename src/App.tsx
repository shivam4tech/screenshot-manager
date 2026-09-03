import { useEffect, useState } from "react";
import { api, type AppStateDto } from "./api";
import { applyTheme, initialTheme, type Theme } from "./theme";
import Onboarding from "./components/Onboarding";
import Library from "./components/Library";
import StatusBar from "./components/StatusBar";

type Screen = "loading" | "onboarding" | "library";

export default function App() {
  const [screen, setScreen] = useState<Screen>("loading");
  const [appState, setAppState] = useState<AppStateDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [theme, setTheme] = useState<Theme>(initialTheme);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  const refresh = async () => {
    try {
      const st = await api.getAppState();
      setAppState(st);
      return st;
    } catch (e) {
      setLoadError(String(e));
      return null;
    }
  };

  useEffect(() => {
    refresh().then((st) => {
      if (st) setScreen(st.onboarded ? "library" : "onboarding");
    });
  }, []);

  if (screen === "loading") {
    return (
      <div className="center-screen">
        {loadError ? (
          <div className="empty-state">
            <h2>Could not start</h2>
            <p>{loadError}</p>
          </div>
        ) : (
          <p className="muted">Loading…</p>
        )}
      </div>
    );
  }

  if (screen === "onboarding") {
    return (
      <Onboarding
        onFinish={async () => {
          const st = await refresh();
          setScreen(st?.onboarded ? "library" : "library");
        }}
      />
    );
  }

  return (
    <div className="app-shell">
      <div className="app-main">
        <Library
          appState={appState}
          theme={theme}
          onToggleTheme={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}
        />
      </div>
      <StatusBar appState={appState} onRefresh={refresh} />
    </div>
  );
}
