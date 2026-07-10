import { Component, type ErrorInfo, type ReactNode } from "react";

import { managerApi } from "../services/managerApi";
import { Ring } from "./components";
import { formatDiagnostics } from "./diagnostics";
import { CATALOG, pickLang, type Lang, type TKey } from "./i18n";

interface State {
  error: Error | null;
  copied: boolean;
  showDetails: boolean;
}

const CRASH_KEYS = [
  "crash.title",
  "crash.body",
  "crash.reload",
  "crash.copy",
  "crash.copied",
  "crash.details",
  "crash.hideDetails",
] as const satisfies readonly TKey[];

type CrashKey = (typeof CRASH_KEYS)[number];

function crashStrings(): Record<CrashKey, string> {
  const saved = typeof localStorage === "undefined" ? null : localStorage.getItem("cam.lang");
  const prefs =
    typeof navigator !== "undefined" && navigator.languages ? Array.from(navigator.languages) : [];
  const lang: Lang = pickLang(saved, prefs);
  const catalog = CATALOG[lang] ?? CATALOG.en;
  const out = {} as Record<CrashKey, string>;
  for (const key of CRASH_KEYS) {
    out[key] = catalog[key] ?? CATALOG.en[key] ?? key;
  }
  return out;
}

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null, copied: false, showDetails: false };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidMount() {
    window.addEventListener("cam:fatal", this.onFatal);
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

  render() {
    if (!this.state.error) {
      return this.props.children;
    }

    const strings = crashStrings();
    const error = this.state.error;
    const summary = `${error.name}: ${error.message}`;

    return (
      <div className="pop">
        <div className="scroll view">
          <section className="hero" style={{ marginTop: 20 }}>
            <Ring icon="alert" variant="danger" />
            <div role="alert" aria-live="assertive">
              <div className="headline">{strings["crash.title"]}</div>
              <div className="desc">{strings["crash.body"]}</div>
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
          </div>
        </div>
      </div>
    );
  }
}
