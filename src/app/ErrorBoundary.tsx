import { Component, type ErrorInfo, type ReactNode } from "react";

import { managerApi } from "../services/managerApi";
import { Ring } from "./components";
import { formatDiagnostics } from "./diagnostics";
import { pickLang } from "./i18n";

const FALLBACK_STRINGS = {
  en: {
    title: "Something went wrong",
    body: "The manager hit an unexpected error. Your Codex install was not changed by this screen.",
    reload: "Reload",
    copy: "Copy diagnostics",
    copied: "Diagnostics copied",
    details: "Show details",
    hideDetails: "Hide details",
  },
  "zh-CN": {
    title: "出了点问题",
    body: "管理器遇到意外错误。此界面不会改动你已安装的 Codex。",
    reload: "重新加载",
    copy: "复制诊断信息",
    copied: "已复制诊断信息",
    details: "查看详情",
    hideDetails: "收起详情",
  },
} as const;

type FallbackLang = keyof typeof FALLBACK_STRINGS;

interface State {
  error: Error | null;
  copied: boolean;
  showDetails: boolean;
}

function fallbackLang(): FallbackLang {
  const saved = typeof localStorage === "undefined" ? null : localStorage.getItem("cam.lang");
  const prefs =
    typeof navigator !== "undefined" && navigator.languages ? Array.from(navigator.languages) : [];
  const code = pickLang(saved, prefs);
  return code in FALLBACK_STRINGS ? (code as FallbackLang) : "en";
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

    const strings = FALLBACK_STRINGS[fallbackLang()];
    const error = this.state.error;
    const summary = `${error.name}: ${error.message}`;

    return (
      <div className="pop">
        <div className="scroll view">
          <section className="hero" style={{ marginTop: 20 }}>
            <Ring icon="alert" variant="danger" />
            <div className="headline">{strings.title}</div>
            <div className="desc">{strings.body}</div>
            <button
              type="button"
              className={`errdetails-toggle${this.state.showDetails ? " open" : ""}`}
              aria-expanded={this.state.showDetails}
              onClick={() => this.setState((state) => ({ showDetails: !state.showDetails }))}
            >
              {this.state.showDetails ? strings.hideDetails : strings.details}
            </button>
            <pre className="errdetails">{summary}</pre>
            {this.state.showDetails && error.stack ? (
              <pre className="errdetails">{error.stack}</pre>
            ) : null}
          </section>
          <div className="actions">
            <button className="btn primary big" onClick={this.reload}>
              {strings.reload}
            </button>
            <button className="btn ghost" onClick={this.copy}>
              {this.state.copied ? strings.copied : strings.copy}
            </button>
          </div>
        </div>
      </div>
    );
  }
}
