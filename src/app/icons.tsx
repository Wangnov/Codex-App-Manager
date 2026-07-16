// Stroke icon set (currentColor, 24×24). The SVG groups carry stable class
// names so parent controls can trigger lightweight CSS animations.

import type { ReactNode } from "react";

import { useTheme } from "./theme";
import logoLight from "./assets/logo-light.png";
import logoDark from "./assets/logo-dark.png";

export type IconName =
  | "check"
  | "alert"
  | "arrowUp"
  | "download"
  | "play"
  | "pause"
  | "loader"
  | "refresh"
  | "gear"
  | "shield"
  | "info"
  | "chevron"
  | "back"
  | "trash"
  | "sliders"
  | "message"
  | "folder"
  | "copy"
  | "globe"
  | "external"
  | "expand"
  | "collapse"
  | "house"
  | "minimize"
  | "close";

const PATHS: Record<IconName, ReactNode> = {
  check: <polyline className="cam-check-mark" points="4 12.5 9.5 18 20 6.5" />,
  alert: (
    <g className="cam-alert">
      <path className="cam-alert-body" d="M12 4 2.6 20h18.8L12 4Z" />
      <line className="cam-alert-line" x1="12" y1="10" x2="12" y2="14" />
      <circle className="cam-alert-dot" cx="12" cy="17.4" r="0.5" />
    </g>
  ),
  arrowUp: (
    <g className="cam-arrow-up">
      <line x1="12" y1="19" x2="12" y2="5.5" />
      <polyline points="6 11 12 5 18 11" />
    </g>
  ),
  download: (
    <>
      <path className="cam-download-stem" d="M12 3.5v11" />
      <polyline className="cam-download-head" points="7.5 10 12 14.5 16.5 10" />
      <path className="cam-download-tray" d="M5 19.5h14" />
    </>
  ),
  play: <path className="cam-play-triangle" d="M8 5.5v13l10-6.5-10-6.5Z" />,
  pause: (
    <>
      <line className="cam-pause-left" x1="9.5" y1="6" x2="9.5" y2="18" />
      <line className="cam-pause-right" x1="14.5" y1="6" x2="14.5" y2="18" />
    </>
  ),
  loader: <path d="M12 3.5a8.5 8.5 0 1 0 8.5 8.5" />,
  refresh: (
    <g className="cam-refresh">
      <path d="M20 11.5a8 8 0 1 0-1.9 6.4" />
      <polyline points="20 4.5 20 11.5 13 11.5" />
    </g>
  ),
  gear: (
    <>
      <path className="cam-gear-body" d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
      <circle className="cam-gear-center" cx="12" cy="12" r="3" />
    </>
  ),
  shield: (
    <g className="cam-shield">
      <path className="cam-shield-body" d="M12 3 5 5.7v5.3c0 4.3 3 7.6 7 9.5 4-1.9 7-5.2 7-9.5V5.7L12 3Z" />
      <polyline className="cam-shield-check" points="9 12 11 14 15 9.6" />
    </g>
  ),
  info: (
    <>
      <circle className="cam-info-circle" cx="12" cy="12" r="8.5" />
      <line className="cam-info-line" x1="12" y1="11" x2="12" y2="16.5" />
      <circle className="cam-info-dot" cx="12" cy="7.6" r="0.5" />
    </>
  ),
  chevron: <polyline className="cam-chevron" points="9 6 15 12 9 18" />,
  back: <polyline className="cam-back" points="15 6 9 12 15 18" />,
  trash: (
    <>
      <path className="cam-trash-lid" d="M5 7h14" />
      <path className="cam-trash-handle" d="M9.5 7V5.2A1.2 1.2 0 0 1 10.7 4h2.6a1.2 1.2 0 0 1 1.2 1.2V7" />
      <path d="M6.5 7l.8 12.1A1.3 1.3 0 0 0 8.6 20.4h6.8a1.3 1.3 0 0 0 1.3-1.3L17.5 7" />
    </>
  ),
  sliders: (
    <>
      <line x1="4" y1="8" x2="20" y2="8" />
      <line x1="4" y1="16" x2="20" y2="16" />
      <circle className="cam-slider-a" cx="9" cy="8" r="2.3" />
      <circle className="cam-slider-b" cx="15" cy="16" r="2.3" />
    </>
  ),
  message: (
    <path className="cam-message" d="M5 5h14a1.5 1.5 0 0 1 1.5 1.5v8A1.5 1.5 0 0 1 19 16H9l-4 3.5V6.5A1.5 1.5 0 0 1 5 5Z" />
  ),
  folder: (
    <>
      <path className="cam-folder-body" d="M4 6.5h6l2 2h8v9A1.5 1.5 0 0 1 18.5 19h-13A1.5 1.5 0 0 1 4 17.5Z" />
      <path className="cam-folder-lid" d="M4 9h16" />
    </>
  ),
  copy: (
    <>
      <path className="cam-copy-back" d="M5 15.5V6.5A1.5 1.5 0 0 1 6.5 5h9" />
      <rect className="cam-copy-front" x="8" y="8" width="11" height="11" rx="1.5" />
    </>
  ),
  globe: (
    <g className="cam-globe">
      <circle cx="12" cy="12" r="8.5" />
      <path d="M3.5 12h17M12 3.5c2.5 2.4 2.5 14.6 0 17M12 3.5c-2.5 2.4-2.5 14.6 0 17" />
    </g>
  ),
  external: (
    <>
      <path className="cam-external-arrow" d="M14 5h5v5" />
      <path className="cam-external-arrow" d="M19 5l-8 8" />
      <path d="M18 14v4.5A1.5 1.5 0 0 1 16.5 20h-11A1.5 1.5 0 0 1 4 18.5v-11A1.5 1.5 0 0 1 5.5 6H10" />
    </>
  ),
  expand: (
    <>
      <g className="cam-expand-ne">
        <polyline points="14.5 4.5 19.5 4.5 19.5 9.5" />
        <line x1="19" y1="5" x2="13.5" y2="10.5" />
      </g>
      <g className="cam-expand-sw">
        <polyline points="9.5 19.5 4.5 19.5 4.5 14.5" />
        <line x1="5" y1="19" x2="10.5" y2="13.5" />
      </g>
    </>
  ),
  collapse: (
    <>
      <g className="cam-collapse-ne">
        <polyline points="19 10 14 10 14 5" />
        <line x1="14.5" y1="9.5" x2="20" y2="4" />
      </g>
      <g className="cam-collapse-sw">
        <polyline points="5 14 10 14 10 19" />
        <line x1="9.5" y1="14.5" x2="4" y2="20" />
      </g>
    </>
  ),
  house: (
    <>
      <path className="cam-house-roof" d="M3.5 10.8 12 3.8l8.5 7" />
      <path d="M6 9.2v9.3A1.5 1.5 0 0 0 7.5 20h9a1.5 1.5 0 0 0 1.5-1.5V9.2" />
    </>
  ),
  minimize: <line className="cam-minimize" x1="6" y1="12" x2="18" y2="12" />,
  close: (
    <>
      <line className="cam-close-a" x1="6" y1="6" x2="18" y2="18" />
      <line className="cam-close-b" x1="18" y1="6" x2="6" y2="18" />
    </>
  ),
};

export function Icon({ name, className }: { name: IconName; className?: string }) {
  return (
    <svg
      className={["cam-icon", `cam-icon-${name}`, className].filter(Boolean).join(" ")}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {PATHS[name]}
    </svg>
  );
}

/** Our own brand mark — the app-icon logo, swapping the Default(light) / Dark
 *  variants with the system theme so it always matches the surrounding chrome.
 *  Sized by its container (`.mark` sets width/height). */
export function CodexMark({ className }: { className?: string }) {
  const { resolved } = useTheme();
  return (
    <img
      className={className}
      src={resolved === "dark" ? logoDark : logoLight}
      alt=""
      aria-hidden="true"
      draggable={false}
    />
  );
}

/** Monochrome Codex glyph (cloud + `>_`, currentColor) — for decorative or
 *  single-color contexts where the white tile would clash. */
export function CodexGlyph({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      width="100%"
      height="100%"
      fill="currentColor"
      fillRule="evenodd"
      clipRule="evenodd"
      aria-hidden="true"
    >
      <path d="M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457zm-.804 7.85a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393zm5.446 6.24a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z" />
    </svg>
  );
}
