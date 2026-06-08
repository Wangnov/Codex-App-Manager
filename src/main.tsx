import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "./app/App";
import { currentPlatform } from "./app/platform";
import "./app/styles.css";

// Tag the platform so the stylesheet can tame Windows' heavier system fonts
// (Segoe UI / Microsoft YaHei) — see the [data-platform="windows"] block.
document.documentElement.dataset.platform = currentPlatform();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

