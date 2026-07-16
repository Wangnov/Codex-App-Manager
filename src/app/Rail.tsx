import { useSyncExternalStore } from "react";

import { CodexMark, Icon, type IconName } from "./icons";
import { useI18n } from "./i18n";
import { navLocked, subscribeNavLock } from "./navLock";
import { useWindowModeOptional } from "./windowMode";

/** Views the rail can jump to directly. Sub-views that live *under* Settings
 *  (About, Uninstall, Codex config) highlight the Settings item — the rail
 *  reflects sections, not the full view stack. */
export type RailSection = "home" | "settings";

/** Left navigation card, shown only in the expanded workbench. It is a second
 *  floating `.pop` card beside the active view's card — same material, same
 *  gutter — so the expanded window reads as "the popover grew a sibling", not
 *  a different app. The whole card is a drag region except its controls. */
export function Rail({
  section,
  onNavigate,
}: {
  section: RailSection;
  onNavigate: (section: RailSection) => void;
}) {
  const { t } = useI18n();
  const windowMode = useWindowModeOptional();
  // An in-flight operation (ProgressScreen mounted) pulls the rail's exits:
  // its compact counterpart offers none, and leaving would allow concurrent
  // operations or unmount the view holding the progress events. Collapsing
  // the window stays allowed — it changes shape, not the mounted view.
  const locked = useSyncExternalStore(subscribeNavLock, navLocked);
  if (windowMode?.mode !== "expanded") return null;

  const items: Array<{ key: RailSection; icon: IconName; label: string }> = [
    { key: "home", icon: "house", label: t("rail.home") },
    { key: "settings", icon: "gear", label: t("nav.settings") },
  ];

  return (
    <aside className="pop rail" data-tauri-drag-region>
      <div className="rail-brand" data-tauri-drag-region>
        <div className="mark" data-tauri-drag-region>
          <CodexMark />
        </div>
        <div className="wordmark" data-tauri-drag-region>
          {t("app.name")}
        </div>
      </div>
      <nav className="rail-nav" aria-label={t("rail.nav")}>
        {items.map((item) => (
          <button
            key={item.key}
            className={`rail-item${section === item.key ? " active" : ""}`}
            aria-current={section === item.key ? "page" : undefined}
            disabled={locked}
            onClick={() => onNavigate(item.key)}
          >
            <Icon name={item.icon} />
            {item.label}
          </button>
        ))}
      </nav>
      <div className="rail-spacer" data-tauri-drag-region />
      <button className="rail-item rail-collapse" onClick={() => windowMode.setMode("compact")}>
        <Icon name="collapse" />
        {t("nav.collapse")}
      </button>
    </aside>
  );
}
