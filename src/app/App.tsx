import { useState } from "react";

import { I18nProvider } from "./i18n";
import { ThemeProvider } from "./theme";
import { withViewTransition } from "./viewTransition";
import { QuitConfirm } from "./components";
import { Home } from "./views/Home";
import { Settings } from "./views/Settings";
import { About } from "./views/About";
import { Uninstall } from "./views/Uninstall";
import { CodexConfig } from "./views/CodexConfig";

type View = "home" | "settings" | "about" | "uninstall" | "config";

function Shell() {
  const [view, setView] = useState<View>("home");

  // Home stays mounted (just hidden) so returning to it doesn't re-mount and
  // re-run the network check — it shows its last state instantly. Sub-views
  // overlay it.
  //
  // That same persistence is why returning to Home has no entrance to play
  // (Home neither re-mounts nor re-keys its GSAP scene), so it would hard-cut.
  // Cross-fade the window instead via the shared ::view-transition(root) rule —
  // no re-mount, no re-check. Forward / inter-sub-view nav keeps each view's own
  // staggered `.view` entrance, so only the return Home is wrapped.
  return (
    <>
      <div style={{ display: view === "home" ? "contents" : "none" }}>
        <Home onOpenSettings={() => setView("settings")} />
      </div>
      {view === "settings" ? (
        <Settings
          onBack={() => withViewTransition(() => setView("home"))}
          onOpenAbout={() => setView("about")}
          onOpenUninstall={() => setView("uninstall")}
          onOpenConfig={() => setView("config")}
        />
      ) : null}
      {view === "about" ? <About onBack={() => setView("settings")} /> : null}
      {view === "uninstall" ? <Uninstall onBack={() => setView("settings")} /> : null}
      {view === "config" ? <CodexConfig onBack={() => setView("settings")} /> : null}
      <QuitConfirm />
    </>
  );
}

export function App() {
  return (
    <ThemeProvider>
      <I18nProvider>
        <Shell />
      </I18nProvider>
    </ThemeProvider>
  );
}
