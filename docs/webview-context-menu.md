# WebView context-menu policy

## Production (release builds)

| Surface | Expected behavior |
| --- | --- |
| Non-editable chrome (home, settings, sheets) | No browser default menu (no Print / Reload / Inspect) |
| Text fields (`input`, `textarea`, `contenteditable`, `role=textbox`) | Copy / cut / paste remain available via keyboard; OS may still show a native edit menu |
| Keyboard menu key / Shift+F10 / trackpad secondary click | Same policy as mouse right-click |

Implementation: `src/app/contextMenuPolicy.ts` (installed from `src/main.tsx` when `import.meta.env.DEV` is false). Capture-phase `contextmenu` + `preventDefault` on non-editable targets. No global disable of accessibility or keyboard shortcuts.

## Development builds

Default browser/WebView context menu stays enabled so Reload and DevTools remain available. The production policy is not installed.

## Manual dual-platform checklist

Run a **release** build (`npm run tauri:build` or CI artifact) on each OS:

1. Right-click empty home chrome → no browser menu.
2. Right-click a settings text field (e.g. custom URL when visible) → edit actions still work; paste with Ctrl/Cmd+V works.
3. Shift+F10 on chrome → no browser menu.
4. Dev build (`npm run tauri:dev`) → context menu still appears for debugging.

Automated coverage: `src/app/contextMenuPolicy.test.ts`.
