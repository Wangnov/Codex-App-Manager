import { Component, type ErrorInfo, type ReactNode } from "react";

type OperationRisk = "unknown" | "idle" | "active" | "paused" | "critical";

interface State {
  error: Error | null;
  operationRisk: OperationRisk;
  quitPending: boolean;
  quitFailure: string | null;
}

interface SnapshotLike {
  kind?: unknown;
  phase?: unknown;
  paused?: unknown;
  interruptible?: unknown;
}

type RootTauriInternals = {
  invoke?: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
};

type RootFrontendReadiness = { generation: number; token: string };

const FRONTEND_READY_EVENT = "cam:frontend-readiness";

function hasRootBackend(): boolean {
  const internals = (
    window as typeof window & { __TAURI_INTERNALS__?: RootTauriInternals }
  ).__TAURI_INTERNALS__;
  return typeof internals?.invoke === "function";
}

function rootFrontendReadiness(): RootFrontendReadiness | null {
  const readiness = (
    window as typeof window & { __CAM_FRONTEND_READY__?: unknown }
  ).__CAM_FRONTEND_READY__;
  if (!readiness || typeof readiness !== "object") return null;
  const { generation, token } = readiness as Partial<RootFrontendReadiness>;
  if (!Number.isSafeInteger(generation) || (generation ?? 0) <= 0) return null;
  if (typeof token !== "string" || !token.trim()) return null;
  return { generation: generation as number, token };
}

function rootErrorMessage(value: unknown): string {
  if (value instanceof Error) return value.message;
  if (value && typeof value === "object" && "message" in value) {
    const message = (value as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return String(value ?? "The backend kept the app open.");
}

/**
 * Minimal IPC path used only when the normal managerApi chunk cannot load.
 * Keeping it here makes the outer boundary independent of the chunk whose
 * failure may have caused the recovery screen in the first place.
 */
export function invokeRootBackend<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const internals = (
    window as typeof window & { __TAURI_INTERNALS__?: RootTauriInternals }
  ).__TAURI_INTERNALS__;
  if (typeof internals?.invoke !== "function") {
    return Promise.reject(new Error("Desktop backend unavailable."));
  }
  try {
    return Promise.resolve(internals.invoke<T>(command, args));
  } catch (cause) {
    return Promise.reject(cause);
  }
}

/**
 * Tell the backend that the current renderer no longer owns native quit-event
 * delivery. If Tauri injects the per-document token after React fails, wait for
 * that event instead of sending an unauthenticated or stale state transition.
 */
export function notifyRootFrontendFailure(): void {
  if (!hasRootBackend()) return;
  const notify = () => {
    const readiness = rootFrontendReadiness();
    if (!readiness) return false;
    void invokeRootBackend<void>("frontend_failed", readiness).catch(() => undefined);
    return true;
  };
  if (notify()) return;
  const onReadiness = () => {
    if (!notify()) return;
    window.removeEventListener(FRONTEND_READY_EVENT, onReadiness);
  };
  window.addEventListener(FRONTEND_READY_EVENT, onReadiness);
}

const COPY = {
  en: {
    title: "The app shell could not start",
    idle: "Reload the interface or quit safely. Your installed Codex was not changed by this screen.",
    active: "A Codex operation is still running in the background. Reloading will reconnect to it.",
    paused: "A paused download is preserved. Reloading will restore its recovery controls.",
    critical: "A protected install step is active. Reloading is safe; quitting will remain blocked until it finishes.",
    reload: "Reload interface",
    quit: "Quit safely",
    detail: "Technical detail",
  },
  zh: {
    title: "应用界面无法启动",
    idle: "你可以重新加载界面或安全退出；此页面不会修改已安装的 Codex。",
    active: "Codex 操作仍在后台运行。重新加载后会自动重新连接。",
    paused: "暂停下载已保留。重新加载后会恢复继续与取消入口。",
    critical: "安装正处于受保护阶段。重新加载是安全的；阶段完成前退出仍会被阻止。",
    reload: "重新加载界面",
    quit: "安全退出",
    detail: "技术详情",
  },
  ar: {
    title: "تعذر بدء واجهة التطبيق",
    idle: "يمكنك إعادة تحميل الواجهة أو الخروج بأمان. لم تغيّر هذه الشاشة تثبيت Codex.",
    active: "لا تزال عملية Codex تعمل في الخلفية. ستعيد الواجهة الاتصال بها بعد إعادة التحميل.",
    paused: "تم الاحتفاظ بالتنزيل المتوقف مؤقتًا. ستعود خيارات الاستئناف والإلغاء بعد إعادة التحميل.",
    critical: "توجد خطوة تثبيت محمية قيد التنفيذ. إعادة التحميل آمنة، وسيظل الخروج محظورًا حتى تنتهي.",
    reload: "إعادة تحميل الواجهة",
    quit: "خروج آمن",
    detail: "تفاصيل تقنية",
  },
} as const;

function toError(value: unknown): Error {
  if (value instanceof Error) return value;
  if (value && typeof value === "object" && "message" in value) {
    const message = (value as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) return new Error(message);
  }
  return new Error(String(value ?? "Unknown root error"));
}

function rootCopy() {
  let saved = "";
  try {
    saved = typeof localStorage === "undefined" ? "" : localStorage.getItem("cam.lang") ?? "";
  } catch {
    // Storage failures can be the reason a provider crashed. The recovery
    // surface must remain renderable without the same dependency.
  }
  const browser = typeof navigator === "undefined" ? "" : navigator.language;
  const lang = (saved || browser).toLowerCase();
  if (lang.startsWith("zh")) return { ...COPY.zh, dir: "ltr" as const };
  if (lang.startsWith("ar")) return { ...COPY.ar, dir: "rtl" as const };
  return { ...COPY.en, dir: "ltr" as const };
}

export function operationRiskForSnapshot(value: unknown): OperationRisk {
  if (!value || typeof value !== "object") return "idle";
  const snapshot = value as SnapshotLike;
  if (snapshot.paused === true) return "paused";
  if (
    snapshot.interruptible === false ||
    snapshot.phase === "committing" ||
    snapshot.phase === "finishing"
  ) {
    return "critical";
  }
  if (
    snapshot.kind === "install" ||
    snapshot.kind === "update" ||
    snapshot.kind === "uninstall" ||
    snapshot.kind === "manager-update"
  ) {
    return "active";
  }
  return "idle";
}

/**
 * Last-resort React boundary. It deliberately avoids providers, icons and the
 * normal i18n catalog so a failure in any of those dependencies cannot turn
 * the recovery surface into a blank window.
 */
export class RootCrashBoundary extends Component<{ children: ReactNode }, State> {
  state: State = {
    error: null,
    operationRisk: "unknown",
    quitPending: false,
    quitFailure: null,
  };

  static getDerivedStateFromError(error: unknown): Partial<State> {
    return { error: toError(error) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    const normalized = toError(error);
    const payload = {
      kind: "root.render",
      message: normalized.message,
      stack: normalized.stack ?? null,
      componentStack: info.componentStack ?? null,
    };
    notifyRootFrontendFailure();
    void import("../services/managerApi")
      .then(async ({ managerApi }) => {
        await managerApi
          .reportFrontendError(payload)
          .catch(() => undefined);
        const snapshot = await managerApi.getOperationSnapshot().catch(() => null);
        this.setState({ operationRisk: operationRiskForSnapshot(snapshot) });
      })
      .catch(async () => {
        await invokeRootBackend<void>("log_frontend_error", { payload }).catch(() => undefined);
        const snapshot = await invokeRootBackend<unknown>("get_operation_snapshot").catch(
          () => null,
        );
        this.setState({ operationRisk: operationRiskForSnapshot(snapshot) });
      });
  }

  private reload = () => {
    window.location.reload();
  };

  private quit = () => {
    if (this.state.quitPending) return;
    this.setState({ quitPending: true, quitFailure: null });
    void import("../services/managerApi")
      .then(async ({ errorMessage, managerApi }) => {
        try {
          await managerApi.confirmQuit();
        } catch (cause) {
          this.setState({
            quitFailure: errorMessage(cause) || "The backend kept the app open.",
          });
        }
      })
      .catch(async () => {
        try {
          await invokeRootBackend<void>("confirm_quit");
        } catch (cause) {
          this.setState({ quitFailure: rootErrorMessage(cause) });
        }
      })
      .finally(() => this.setState({ quitPending: false }));
  };

  render() {
    if (!this.state.error) return this.props.children;

    const copy = rootCopy();
    const risk = this.state.operationRisk === "unknown" ? "idle" : this.state.operationRisk;
    return (
      <main className="pop" data-root-crash="true" dir={copy.dir}>
        <div className="scroll view">
          <section className="hero" style={{ marginTop: 20 }}>
            <div role="alert" aria-live="assertive">
              <h1 className="headline">{copy.title}</h1>
              <p className="desc">{copy[risk]}</p>
            </div>
            <details>
              <summary>{copy.detail}</summary>
              <pre className="errdetails">
                {this.state.error.name}: {this.state.error.message}
              </pre>
            </details>
          </section>
          <div className="actions">
            <button type="button" className="btn primary big" onClick={this.reload}>
              {copy.reload}
            </button>
            <button
              type="button"
              className="btn ghost"
              onClick={this.quit}
              disabled={this.state.quitPending}
            >
              {copy.quit}
            </button>
            {this.state.quitFailure ? (
              <p className="desc" role="alert">
                {this.state.quitFailure}
              </p>
            ) : null}
          </div>
        </div>
      </main>
    );
  }
}
