import { Component, type ErrorInfo, type ReactNode } from "react";

import { managerApi } from "../services/managerApi";
import type { OperationSnapshot } from "../shared/types";
import { Ring } from "./components";
import { formatDiagnostics } from "./diagnostics";
import { CATALOG, pickLang, type Lang, type TKey } from "./i18n";

interface State {
  error: Error | null;
  copied: boolean;
  showDetails: boolean;
  /** Backend operation lease, if any — drives crash-page copy. */
  opSnapshot: OperationSnapshot | null;
  opLoaded: boolean;
}

const CRASH_KEYS = [
  "crash.title",
  "crash.body",
  "crash.bodyActive",
  "crash.bodyCritical",
  "crash.bodyPaused",
  "crash.reload",
  "crash.copy",
  "crash.copied",
  "crash.details",
  "crash.hideDetails",
  "crash.quit",
] as const satisfies readonly TKey[];

type CrashKey = (typeof CRASH_KEYS)[number];
export type CrashStrings = Record<CrashKey, string>;

function crashStrings(): CrashStrings {
  const saved = typeof localStorage === "undefined" ? null : localStorage.getItem("cam.lang");
  const prefs =
    typeof navigator !== "undefined" && navigator.languages ? Array.from(navigator.languages) : [];
  const lang: Lang = pickLang(saved, prefs);
  const catalog = CATALOG[lang] ?? CATALOG.en;
  const out = {} as CrashStrings;
  for (const key of CRASH_KEYS) {
    out[key] = catalog[key] ?? CATALOG.en[key] ?? key;
  }
  return out;
}

/** Exported for unit tests — pick crash-page body from a backend snapshot. */
export function crashBodyForSnapshot(
  strings: CrashStrings,
  snap: OperationSnapshot | null,
): string {
  if (!snap) return strings["crash.body"];
  if (snap.paused) return strings["crash.bodyPaused"];
  if (snap.phase === "committing" || snap.phase === "finishing" || !snap.interruptible) {
    return strings["crash.bodyCritical"];
  }
  if (snap.kind === "install" || snap.kind === "update") {
    return strings["crash.bodyActive"];
  }
  return strings["crash.body"];
}

function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
  );
}

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = {
    error: null,
    copied: false,
    showDetails: false,
    opSnapshot: null,
    opLoaded: false,
  };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidMount() {
    window.addEventListener("cam:fatal", this.onFatal);
    // Child may have thrown during the first render (getDerivedStateFromError),
    // so the crash screen can be the first committed state — load snapshot now.
    if (this.state.error) {
      this.loadOperationSnapshot();
    }
  }

  componentDidUpdate(_prevProps: { children: ReactNode }, prevState: State) {
    // cam:fatal / late throw: enter crash screen after a healthy mount.
    if (this.state.error && !prevState.error) {
      this.loadOperationSnapshot();
    }
  }

  componentWillUnmount() {
    window.removeEventListener("cam:fatal", this.onFatal);
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    void managerApi.reportFrontendError({
      kind: "render",
      message: error.message,
      stack: error.stack ?? null,
      componentStack: info.componentStack ?? null,
    });
  }

  private loadOperationSnapshot = () => {
    void Promise.resolve()
      .then(() => managerApi.getOperationSnapshot())
      .then((snap) => {
        this.setState({ opSnapshot: snap, opLoaded: true });
      })
      .catch(() => {
        this.setState({ opSnapshot: null, opLoaded: true });
      });
  };

  private onFatal = (event: Event) => {
    const detail = (event as CustomEvent<{ error?: unknown }>).detail;
    const incoming = detail?.error;
    const error =
      incoming instanceof Error
        ? incoming
        : incoming == null
          ? null
          : new Error(String(incoming));
    if (!this.state.error && error) {
      this.setState({ error });
    }
  };

  private reload = () => {
    window.location.reload();
  };

  private copy = async () => {
    try {
      const diagnostics = await managerApi.getDiagnostics();
      await navigator.clipboard.writeText(formatDiagnostics(diagnostics, this.state.error));
      this.setState({ copied: true });
    } catch {
      // The crash screen must never throw while trying to help.
    }
  };

  /** Ask the shared QuitConfirm (mounted outside this boundary) to handle exit. */
  private requestQuit = () => {
    if (isTauri()) {
      void import("@tauri-apps/api/window")
        .then(({ getCurrentWindow }) => getCurrentWindow().close())
        .catch(() => {
          void managerApi.confirmQuit().catch(() => undefined);
        });
    } else {
      window.dispatchEvent(new Event("cam:confirm-quit"));
    }
  };

  render() {
    if (!this.state.error) {
      return this.props.children;
    }

    const strings = crashStrings();
    const error = this.state.error;
    const summary = `${error.name}: ${error.message}`;
    const body = this.state.opLoaded
      ? crashBodyForSnapshot(strings, this.state.opSnapshot)
      : strings["crash.body"];

    return (
      <div className="pop">
        <div className="scroll view">
          <section className="hero" style={{ marginTop: 20 }}>
            <Ring icon="alert" variant="danger" />
            <div role="alert" aria-live="assertive">
              <div className="headline">{strings["crash.title"]}</div>
              <div className="desc">{body}</div>
            </div>
            <button
              type="button"
              className={`errdetails-toggle${this.state.showDetails ? " open" : ""}`}
              aria-expanded={this.state.showDetails}
              onClick={() => this.setState((state) => ({ showDetails: !state.showDetails }))}
            >
              {this.state.showDetails ? strings["crash.hideDetails"] : strings["crash.details"]}
            </button>
            {this.state.showDetails ? (
              <div className="errdetails-panel open">
                <div className="errdetails-panel-inner">
                  <pre className="errdetails">{summary}</pre>
                  {error.stack ? <pre className="errdetails">{error.stack}</pre> : null}
                </div>
              </div>
            ) : null}
          </section>
          <div className="actions">
            <button className="btn primary big" onClick={this.reload}>
              {strings["crash.reload"]}
            </button>
            <button className="btn ghost" onClick={this.copy}>
              {this.state.copied ? strings["crash.copied"] : strings["crash.copy"]}
            </button>
            <button className="btn ghost" onClick={this.requestQuit}>
              {strings["crash.quit"]}
            </button>
          </div>
        </div>
      </div>
    );
  }
}
