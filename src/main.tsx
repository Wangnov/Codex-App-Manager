import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "./app/App";
import { installContextMenuPolicy } from "./app/contextMenuPolicy";
import { ErrorBoundary } from "./app/ErrorBoundary";
import { installGlobalErrorHandlers } from "./app/globalErrors";
import { currentPlatform } from "./app/platform";
import { resolveInitialTheme } from "./app/theme";
import "./app/styles.css";

// Tag the platform so the stylesheet can tame Windows' heavier system fonts
// (Segoe UI / Microsoft YaHei) — see the [data-platform="windows"] block.
document.documentElement.dataset.platform = currentPlatform();
// Resolve the theme BEFORE first paint. The ThemeProvider re-derives (and
// keeps) this value, but it lands in a passive effect — after paint — and the
// bare `:root` defaults to the dark tokens, so without this a light-theme
// user gets one dark frame on every launch.
document.documentElement.dataset.theme = resolveInitialTheme();
installGlobalErrorHandlers();
// Production WebViews: no browser Print/Reload menu; editable fields keep
// copy/paste. Dev builds leave the default menu for Reload/DevTools.
installContextMenuPolicy();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <ErrorBoundary>
    <React.StrictMode>
      <App />
    </React.StrictMode>
  </ErrorBoundary>,
);
