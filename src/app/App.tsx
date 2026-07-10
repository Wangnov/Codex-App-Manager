import { useEffect, useRef, useState } from "react";

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

function focusPageTarget(root: ParentNode | null) {
  if (!root) return;
  const active = document.activeElement;
  if (active && active !== document.body && active !== document.documentElement) {
    if (root instanceof Element && root.contains(active)) return;
    // Focus is still on a control from a view that just unmounted — reclaim it.
  }
  const preferred =
    (root as Element).querySelector?.<HTMLElement>("[data-page-focus]") ??
    (root as Element).querySelector?.<HTMLElement>(
      'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
    );
  preferred?.focus({ preventScroll: true });
}

function Shell() {
  const [view, setView] = useState<View>("home");
  // Skip the first paint: NavBar / Home already own initial focus; stealing it
  // on mount is noisier than helpful for keyboard users.
  const skipInitialFocus = useRef(true);

  // Home stays mounted (just hidden) so returning to it doesn't re-mount and
  // re-run the network check — it shows its last state instantly. Sub-views
  // overlay it.
  //
  // That same persistence is why returning to Home has no entrance to play
  // (Home neither re-mounts nor re-keys its GSAP scene), so it would hard-cut.
  // Cross-fade the window instead via the shared ::view-transition(root) rule —
  // no re-mount, no re-check. Forward / inter-sub-view nav keeps each view's own
  // staggered `.view` entrance, so only the return Home is wrapped.
  //
  // After a view change, ensure keyboard focus has a definite landing target
  // (NavBar focuses its back control; Home focuses the settings control).
  // Scope by data-view so we never hit the hidden Home .pop while a sub-view
  // is showing (querySelector(".pop") would prefer the still-mounted Home).
  useEffect(() => {
    if (skipInitialFocus.current) {
      skipInitialFocus.current = false;
      return;
    }
    const id = window.requestAnimationFrame(() => {
      focusPageTarget(document.querySelector(`[data-view="${view}"]`));
    });
    return () => window.cancelAnimationFrame(id);
  }, [view]);

  return (
    <>
      <div data-view="home" style={{ display: view === "home" ? "contents" : "none" }}>
        <Home onOpenSettings={() => setView("settings")} />
      </div>
      {view === "settings" ? (
        <div data-view="settings" style={{ display: "contents" }}>
          <Settings
            onBack={() => withViewTransition(() => setView("home"))}
            onOpenAbout={() => setView("about")}
            onOpenUninstall={() => setView("uninstall")}
            onOpenConfig={() => setView("config")}
          />
        </div>
      ) : null}
      {view === "about" ? (
        <div data-view="about" style={{ display: "contents" }}>
          <About onBack={() => setView("settings")} />
        </div>
      ) : null}
      {view === "uninstall" ? (
        <div data-view="uninstall" style={{ display: "contents" }}>
          <Uninstall onBack={() => setView("settings")} />
        </div>
      ) : null}
      {view === "config" ? (
        <div data-view="config" style={{ display: "contents" }}>
          <CodexConfig onBack={() => setView("settings")} />
        </div>
      ) : null}
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
