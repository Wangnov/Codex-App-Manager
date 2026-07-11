import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";

import {
  errorCode,
  managerApi,
  type ManagerUpdateAvailable,
} from "../services/managerApi";
import type {
  ManagerUpdateProgress,
  ManagerUpdateRuntimeSnapshot,
} from "../shared/types";
import { FailureBanner, Ring, StatusBanner } from "./components";
import { resolveFailure, type FailureSurface } from "./errorCopy";
import { Icon } from "./icons";
import { useI18n } from "./i18n";
import { Sheet } from "./Sheet";

export const MANAGER_UPDATE_STATE_EVENT = "manager://update-state";
export const MANAGER_UPDATE_STARTUP_DELAY_MS = 1_500;
export const MANAGER_UPDATE_PERIODIC_INTERVAL_MS = 6 * 60 * 60 * 1_000;
export const MANAGER_UPDATE_SNOOZE_MS = 24 * 60 * 60 * 1_000;
export const MANAGER_UPDATE_SNOOZE_KEY = "cam.manager-update-snooze";
export const MANAGER_UPDATE_COMPLETION_KEY = "cam.manager-update-completion";
export const MANAGER_UPDATE_HANDOFF_GRACE_MS = 10 * 60 * 1_000;

const APP_VERSION = import.meta.env.VITE_APP_VERSION ?? "0.0.0";

type ManagerUpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "up-to-date"
  | "development"
  | "downloading"
  | "installing"
  | "relaunching"
  | "installed-awaiting-relaunch"
  | "error";

type CheckOptions = {
  manual?: boolean;
  openWhenAvailable?: boolean;
};

type Completion = {
  from: string;
  to: string;
  installedAt: number;
  /**
   * `downloading` is written before invoking the updater and must never be
   * treated as proof that Windows handed off to NSIS. `installing` is written
   * only after the backend reports that no-return boundary. Seeing the target
   * version on the next launch is the durable proof that the handoff completed.
   *
   * Markers without a stage come from older builds and are treated as
   * `installed` for backwards compatibility.
   */
  stage?: "downloading" | "installing" | "installed";
  /** Backend-confirmed durable Windows NSIS handoff time. */
  handoffStartedAt?: number;
};

type Snooze = {
  version: string;
  remindAt: number;
};

interface ManagerUpdateContextValue {
  status: ManagerUpdateStatus;
  update: ManagerUpdateAvailable | null;
  progress: ManagerUpdateProgress | null;
  failure: FailureSurface | null;
  detailsOpen: boolean;
  snoozed: boolean;
  completion: Completion | null;
  check: (options?: CheckOptions) => Promise<void>;
  openDetails: () => void;
  closeDetails: () => void;
  remindLater: () => void;
  install: () => Promise<void>;
  retryRelaunch: () => Promise<void>;
  dismissCompletion: () => void;
}

const ManagerUpdateContext = createContext<ManagerUpdateContextValue | null>(null);

function readJson<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

function writeJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // A disabled/full localStorage must not block a signed update or relaunch.
  }
}

function removeStored(key: string) {
  try {
    localStorage.removeItem(key);
  } catch {
    // Best effort only.
  }
}

function completionStateForVersion(currentVersion: string): {
  completion: Completion | null;
  awaitingUpdate: ManagerUpdateAvailable | null;
  provisional: Completion | null;
} {
  const completion = readJson<Completion>(MANAGER_UPDATE_COMPLETION_KEY);
  if (!completion) return { completion: null, awaitingUpdate: null, provisional: null };
  if (completion.to === currentVersion) {
    // This process is already the requested target. That is stronger evidence
    // than the old process receiving an install result (which never happens on
    // Windows because the updater exits it after handing off to NSIS).
    return {
      completion: { ...completion, stage: "installed" },
      awaitingUpdate: null,
      provisional: null,
    };
  }
  // The updater committed a replacement but the old process did not relaunch.
  // Restoring this state prevents a reopened old build from installing again.
  if (completion.from === currentVersion && completion.to !== currentVersion) {
    if (completion.stage === "installing") {
      const handoffStartedAt = Number(completion.handoffStartedAt);
      if (Number.isFinite(handoffStartedAt) && handoffStartedAt > 0) {
        // Only Windows persists a backend-confirmed NSIS handoff across process
        // exit. A renderer reload on any platform still recovers from the live
        // runtime below, but a cold start may trust this marker only when the
        // backend supplied durable evidence.
        return { completion: null, awaitingUpdate: null, provisional: completion };
      }
      removeStored(MANAGER_UPDATE_COMPLETION_KEY);
      return { completion: null, awaitingUpdate: null, provisional: null };
    }
    if (completion.stage === "downloading") {
      // A process can exit or crash before Update::install reaches its Windows
      // installer handoff. The backend runtime is process-local, so a fresh
      // process must not turn this pre-handoff marker into a grace-period lock.
      removeStored(MANAGER_UPDATE_COMPLETION_KEY);
      return { completion: null, awaitingUpdate: null, provisional: null };
    }
    return {
      completion: null,
      awaitingUpdate: {
        kind: "available",
        version: completion.to,
        currentVersion: completion.from,
      },
      provisional: null,
    };
  }
  // Unrelated/malformed markers must never claim this build completed.
  removeStored(MANAGER_UPDATE_COMPLETION_KEY);
  return { completion: null, awaitingUpdate: null, provisional: null };
}

function snoozedFor(update: ManagerUpdateAvailable | null): boolean {
  if (!update) return false;
  const snooze = readJson<Snooze>(MANAGER_UPDATE_SNOOZE_KEY);
  if (!snooze || snooze.version !== update.version) return false;
  if (snooze.remindAt <= Date.now()) {
    removeStored(MANAGER_UPDATE_SNOOZE_KEY);
    return false;
  }
  return true;
}

function contextualRelaunchFailure(
  cause: unknown,
  message: string,
  t: ReturnType<typeof useI18n>["t"],
): FailureSurface {
  const failure = resolveFailure(cause, t);
  return {
    ...failure,
    code: "manager_relaunch_failed",
    message,
    recoverable: true,
  };
}

export function ManagerUpdateProvider({
  children,
  currentVersion = APP_VERSION,
  startupDelayMs = MANAGER_UPDATE_STARTUP_DELAY_MS,
  periodicIntervalMs = MANAGER_UPDATE_PERIODIC_INTERVAL_MS,
}: {
  children: ReactNode;
  currentVersion?: string;
  startupDelayMs?: number;
  periodicIntervalMs?: number;
}) {
  const { t } = useI18n();
  const [initialCompletion] = useState(() => completionStateForVersion(currentVersion));
  const [status, setStatus] = useState<ManagerUpdateStatus>(() =>
    initialCompletion.awaitingUpdate
      ? "installed-awaiting-relaunch"
      : initialCompletion.provisional
        ? "installing"
        : "idle",
  );
  const [update, setUpdate] = useState<ManagerUpdateAvailable | null>(
    initialCompletion.awaitingUpdate ??
      (initialCompletion.provisional
        ? {
            kind: "available",
            version: initialCompletion.provisional.to,
            currentVersion: initialCompletion.provisional.from,
          }
        : null),
  );
  const [progress, setProgress] = useState<ManagerUpdateProgress | null>(
    initialCompletion.provisional
      ? { phase: "installing", downloaded: 0, total: null }
      : null,
  );
  const [failure, setFailure] = useState<FailureSurface | null>(null);
  const [detailsOpen, setDetailsOpen] = useState(Boolean(initialCompletion.provisional));
  const [snoozed, setSnoozed] = useState(false);
  const [completion, setCompletion] = useState<Completion | null>(
    initialCompletion.completion,
  );
  const [runtimeHydrated, setRuntimeHydrated] = useState(false);
  const checkInFlightRef = useRef<Promise<void> | null>(null);
  const installInFlightRef = useRef(Boolean(initialCompletion.provisional));
  const ownsInstallRef = useRef(false);
  const lastRuntimeRevisionRef = useRef(0);
  const lastRuntimeSnapshotRef = useRef<ManagerUpdateRuntimeSnapshot | null>(null);
  const recoveredRuntimeRef = useRef(false);
  const recoveredInstalledRuntimeRef = useRef(false);
  const automaticRelaunchAttemptedRef = useRef(false);
  // Once the updater has committed the new package, this process must never
  // check or install again. Only a relaunch can load the newly installed build.
  const installedAwaitingRelaunchRef = useRef(Boolean(initialCompletion.awaitingUpdate));
  const updateRef = useRef(update);
  updateRef.current = update;

  useEffect(() => {
    // The upgraded process owns the in-memory success banner. Consume the
    // durable handoff marker now so a later app launch cannot repeat it.
    if (initialCompletion.completion) {
      removeStored(MANAGER_UPDATE_COMPLETION_KEY);
    }
  }, [initialCompletion.completion]);

  const check = useCallback(
    (options: CheckOptions = {}) => {
      if (installInFlightRef.current || installedAwaitingRelaunchRef.current) {
        return Promise.resolve();
      }
      if (checkInFlightRef.current) return checkInFlightRef.current;

      const task = (async () => {
        const runtimeRevisionAtStart = lastRuntimeRevisionRef.current;
        setStatus("checking");
        if (options.manual) setFailure(null);
        try {
          const result = await managerApi.checkManagerUpdate();
          if (
            lastRuntimeRevisionRef.current !== runtimeRevisionAtStart ||
            installInFlightRef.current ||
            installedAwaitingRelaunchRef.current
          ) {
            return;
          }
          setFailure(null);
          if (result.kind === "available") {
            setUpdate(result);
            setSnoozed(snoozedFor(result));
            setStatus("available");
            if (options.openWhenAvailable) setDetailsOpen(true);
          } else {
            setUpdate(null);
            setSnoozed(false);
            setStatus(result.kind === "development" ? "development" : "up-to-date");
          }
        } catch (cause) {
          if (
            lastRuntimeRevisionRef.current !== runtimeRevisionAtStart ||
            installInFlightRef.current ||
            installedAwaitingRelaunchRef.current
          ) {
            return;
          }
          const next = resolveFailure(cause, t);
          setStatus(options.manual ? "error" : updateRef.current ? "available" : "idle");
          setFailure(next);
          if (options.manual) setDetailsOpen(true);
        }
      })().finally(() => {
        checkInFlightRef.current = null;
      });
      checkInFlightRef.current = task;
      return task;
    },
    [t],
  );

  const applyRuntimeSnapshot = useCallback(
    (snapshot: ManagerUpdateRuntimeSnapshot) => {
      if (
        snapshot.currentVersion !== currentVersion ||
        snapshot.revision <= lastRuntimeRevisionRef.current
      ) {
        return;
      }
      lastRuntimeRevisionRef.current = snapshot.revision;
      lastRuntimeSnapshotRef.current = snapshot;
      const available: ManagerUpdateAvailable = {
        kind: "available",
        version: snapshot.version,
        currentVersion: snapshot.currentVersion,
        body: snapshot.body,
      };
      setUpdate(available);
      setSnoozed(false);

      if (snapshot.phase === "downloading" || snapshot.phase === "installing") {
        installInFlightRef.current = true;
        installedAwaitingRelaunchRef.current = false;
        writeJson(MANAGER_UPDATE_COMPLETION_KEY, {
          from: snapshot.currentVersion,
          to: snapshot.version,
          installedAt: Date.now(),
          stage: snapshot.phase,
          ...(snapshot.phase === "installing" && snapshot.handoffStartedAt
            ? { handoffStartedAt: snapshot.handoffStartedAt }
            : {}),
        } satisfies Completion);
        setFailure(null);
        setProgress({
          phase: snapshot.phase,
          downloaded: snapshot.downloaded,
          total: snapshot.total,
        });
        setStatus(snapshot.phase);
        setDetailsOpen(true);
        return;
      }

      if (snapshot.phase === "installed") {
        installedAwaitingRelaunchRef.current = true;
        if (!ownsInstallRef.current) {
          installInFlightRef.current = false;
          recoveredInstalledRuntimeRef.current = true;
        }
        const marker: Completion = {
          from: snapshot.currentVersion,
          to: snapshot.version,
          installedAt: Date.now(),
          stage: "installed",
        };
        writeJson(MANAGER_UPDATE_COMPLETION_KEY, marker);
        removeStored(MANAGER_UPDATE_SNOOZE_KEY);
        setProgress({
          phase: "installed",
          downloaded: snapshot.downloaded,
          total: snapshot.total,
        });
        setFailure(null);
        setStatus("installed-awaiting-relaunch");
        setDetailsOpen(true);
        return;
      }

      installInFlightRef.current = false;
      ownsInstallRef.current = false;
      installedAwaitingRelaunchRef.current = false;
      removeStored(MANAGER_UPDATE_COMPLETION_KEY);
      setProgress(null);
      setFailure(
        resolveFailure(
          snapshot.failure ?? {
            code: "engine_error",
            message: "manager update failed",
          },
          t,
        ),
      );
      setStatus("error");
      setDetailsOpen(true);
    },
    [currentVersion, t],
  );

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    let pollTimer: number | null = null;
    let awaitingHydration = true;
    let hydrationFailures = 0;
    const provisionalMarker =
      initialCompletion.provisional?.from === currentVersion
        ? initialCompletion.provisional
        : null;
    const hasProvisionalMarker = Boolean(provisionalMarker);
    const hydrationStartedAt = Date.now();
    const markerInstalledAt = Number(provisionalMarker?.handoffStartedAt);
    const markerAgeAtHydration =
      Number.isFinite(markerInstalledAt) && markerInstalledAt > 0
        ? Math.max(0, hydrationStartedAt - markerInstalledAt)
        : 0;
    const provisionalDeadline =
      hydrationStartedAt + Math.max(0, MANAGER_UPDATE_HANDOFF_GRACE_MS - markerAgeAtHydration);
    const isActive = (snapshot: ManagerUpdateRuntimeSnapshot | null) =>
      snapshot?.phase === "downloading" || snapshot?.phase === "installing";

    function clearPoll() {
      if (pollTimer != null) {
        window.clearTimeout(pollTimer);
        pollTimer = null;
      }
    }

    function finishHydration() {
      if (!awaitingHydration || disposed) return;
      awaitingHydration = false;
      setRuntimeHydrated(true);
    }

    function provisionalGraceRemaining() {
      if (!provisionalMarker) return 0;
      return Math.max(0, provisionalDeadline - Date.now());
    }

    function releaseProvisionalHandoff() {
      removeStored(MANAGER_UPDATE_COMPLETION_KEY);
      installInFlightRef.current = false;
      ownsInstallRef.current = false;
      setUpdate(null);
      setProgress(null);
      setFailure(null);
      setStatus("idle");
      setDetailsOpen(false);
    }

    async function queryRuntime() {
      let next: ManagerUpdateRuntimeSnapshot | null = null;
      try {
        next = await managerApi.getManagerUpdateRuntime();
      } catch {
        if (disposed) return;
        if (awaitingHydration) {
          hydrationFailures += 1;
          if (hasProvisionalMarker && lastRuntimeRevisionRef.current === 0) {
            const remaining = provisionalGraceRemaining();
            if (remaining > 0) {
              schedulePoll(Math.min(800, remaining));
              return;
            }
            releaseProvisionalHandoff();
            finishHydration();
          } else if (
            isActive(lastRuntimeSnapshotRef.current) ||
            hydrationFailures < 3
          ) {
            schedulePoll();
          } else {
            // With no durable evidence of an updater handoff, do not disable
            // normal checks forever merely because the runtime query failed.
            finishHydration();
          }
        } else if (isActive(lastRuntimeSnapshotRef.current)) {
          schedulePoll();
        }
        return;
      }
      if (disposed) return;
      const relevant = next?.currentVersion === currentVersion ? next : null;
      if (relevant) {
        recoveredRuntimeRef.current = true;
        applyRuntimeSnapshot(relevant);
      }
      if (
        awaitingHydration &&
        hasProvisionalMarker &&
        lastRuntimeRevisionRef.current === 0
      ) {
        const remaining = provisionalGraceRemaining();
        if (remaining > 0) {
          // A Windows process can be reopened after Tauri has handed the NSIS
          // child off and exited. Its fresh marker is the only cross-process
          // evidence during that gap, so retain it and keep install controls
          // fenced until the installer has had a bounded chance to finish.
          schedulePoll(remaining);
          return;
        }
        releaseProvisionalHandoff();
      }
      finishHydration();
      if (isActive(relevant ?? lastRuntimeSnapshotRef.current)) schedulePoll();
    }

    function schedulePoll(delayMs = 800) {
      clearPoll();
      pollTimer = window.setTimeout(() => {
        void queryRuntime();
      }, delayMs);
    }

    void (async () => {
      try {
        const fn = await listen<ManagerUpdateRuntimeSnapshot>(
          MANAGER_UPDATE_STATE_EVENT,
          (event) => {
            if (!disposed) {
              if (event.payload.currentVersion === currentVersion) {
                recoveredRuntimeRef.current = true;
              }
              applyRuntimeSnapshot(event.payload);
            }
          },
        );
        if (disposed) {
          fn();
          return;
        }
        unlisten = fn;
      } catch {
        // Browser preview and an already-closing WebView have no event bus.
      }
      await queryRuntime();
    })();
    return () => {
      disposed = true;
      clearPoll();
      unlisten?.();
    };
  }, [applyRuntimeSnapshot, currentVersion, initialCompletion.provisional]);

  useEffect(() => {
    if (!runtimeHydrated) return;
    const startup = recoveredRuntimeRef.current
      ? null
      : window.setTimeout(() => void check(), Math.max(0, startupDelayMs));
    const periodic = window.setInterval(
      () => void check(),
      Math.max(60_000, periodicIntervalMs),
    );
    return () => {
      if (startup != null) window.clearTimeout(startup);
      window.clearInterval(periodic);
    };
  }, [check, periodicIntervalMs, runtimeHydrated, startupDelayMs]);

  const openDetails = useCallback(() => setDetailsOpen(true), []);
  const closeDetails = useCallback(() => {
    if (installInFlightRef.current) return;
    setDetailsOpen(false);
  }, []);

  const remindLater = useCallback(() => {
    const current = updateRef.current;
    if (
      !current ||
      installInFlightRef.current ||
      installedAwaitingRelaunchRef.current
    ) {
      return;
    }
    writeJson(MANAGER_UPDATE_SNOOZE_KEY, {
      version: current.version,
      remindAt: Date.now() + MANAGER_UPDATE_SNOOZE_MS,
    } satisfies Snooze);
    setSnoozed(true);
    setDetailsOpen(false);
  }, []);

  const acknowledgeTerminalRuntime = useCallback(
    async (expectedFailureCode?: string) => {
      try {
        const snapshot = await managerApi.getManagerUpdateRuntime();
        if (
          !snapshot ||
          (snapshot.phase !== "installed" && snapshot.phase !== "error") ||
          (expectedFailureCode && snapshot.failure?.code !== expectedFailureCode)
        ) {
          return false;
        }
        const acknowledged = await managerApi.acknowledgeManagerUpdateRuntime(snapshot);
        if (acknowledged) {
          // Fence out an already-queued terminal event for the state we just
          // consumed. A subsequent update begins at a higher backend revision.
          lastRuntimeRevisionRef.current = Math.max(
            lastRuntimeRevisionRef.current,
            snapshot.revision,
          );
          if (lastRuntimeSnapshotRef.current?.revision === snapshot.revision) {
            lastRuntimeSnapshotRef.current = null;
          }
        }
        return acknowledged;
      } catch {
        return false;
      }
    },
    [],
  );

  const retryRelaunch = useCallback(async () => {
    if (installInFlightRef.current || !installedAwaitingRelaunchRef.current) return;
    installInFlightRef.current = true;
    setFailure(null);
    setStatus("relaunching");
    try {
      await managerApi.relaunchManager();
    } catch (cause) {
      setFailure(contextualRelaunchFailure(cause, t("managerUpdate.relaunchFailed"), t));
      setStatus("installed-awaiting-relaunch");
      setDetailsOpen(true);
      installInFlightRef.current = false;
      ownsInstallRef.current = false;
    }
  }, [t]);

  useEffect(() => {
    if (
      !runtimeHydrated ||
      status !== "installed-awaiting-relaunch" ||
      !recoveredInstalledRuntimeRef.current ||
      ownsInstallRef.current ||
      automaticRelaunchAttemptedRef.current
    ) {
      return;
    }
    automaticRelaunchAttemptedRef.current = true;
    void retryRelaunch();
  }, [retryRelaunch, runtimeHydrated, status]);

  const install = useCallback(async () => {
    const current = updateRef.current;
    if (
      !current ||
      installInFlightRef.current ||
      installedAwaitingRelaunchRef.current
    ) {
      return;
    }
    installInFlightRef.current = true;
    ownsInstallRef.current = true;
    setDetailsOpen(true);
    setFailure(null);
    setProgress({ phase: "downloading", downloaded: 0, total: null });
    setStatus("downloading");
    let installed = false;
    const pendingMarker: Completion = {
      from: current.currentVersion,
      to: current.version,
      installedAt: Date.now(),
      stage: "downloading",
    };
    // Persist the active download before invoking the updater. The backend
    // runtime event promotes this to `installing` only when Windows reaches the
    // NSIS handoff; code after the await is a macOS/Linux path.
    writeJson(MANAGER_UPDATE_COMPLETION_KEY, pendingMarker);
    try {
      await managerApi.installManagerUpdate(current);
      installed = true;
      installedAwaitingRelaunchRef.current = true;
      const marker: Completion = {
        ...pendingMarker,
        installedAt: Date.now(),
        stage: "installed",
      };
      writeJson(MANAGER_UPDATE_COMPLETION_KEY, marker);
      removeStored(MANAGER_UPDATE_SNOOZE_KEY);
      setStatus("relaunching");
      await managerApi.relaunchManager();
    } catch (cause) {
      if (installed) {
        setFailure(contextualRelaunchFailure(cause, t("managerUpdate.relaunchFailed"), t));
        setStatus("installed-awaiting-relaunch");
      } else if (errorCode(cause) === "stale_expectation") {
        removeStored(MANAGER_UPDATE_COMPLETION_KEY);
        installInFlightRef.current = false;
        ownsInstallRef.current = false;
        setProgress(null);
        setUpdate(null);
        await acknowledgeTerminalRuntime("stale_expectation");
        await check({ manual: true, openWhenAvailable: true });
        return;
      } else {
        removeStored(MANAGER_UPDATE_COMPLETION_KEY);
        setFailure(resolveFailure(cause, t));
        setStatus("error");
      }
      installInFlightRef.current = false;
      ownsInstallRef.current = false;
    }
  }, [acknowledgeTerminalRuntime, check, t]);

  const dismissCompletion = useCallback(() => {
    removeStored(MANAGER_UPDATE_COMPLETION_KEY);
    setCompletion(null);
    void acknowledgeTerminalRuntime();
  }, [acknowledgeTerminalRuntime]);

  const value = useMemo<ManagerUpdateContextValue>(
    () => ({
      status,
      update,
      progress,
      failure,
      detailsOpen,
      snoozed,
      completion,
      check,
      openDetails,
      closeDetails,
      remindLater,
      install,
      retryRelaunch,
      dismissCompletion,
    }),
    [
      status,
      update,
      progress,
      failure,
      detailsOpen,
      snoozed,
      completion,
      check,
      openDetails,
      closeDetails,
      remindLater,
      install,
      retryRelaunch,
      dismissCompletion,
    ],
  );

  return <ManagerUpdateContext.Provider value={value}>{children}</ManagerUpdateContext.Provider>;
}

export function useManagerUpdate(): ManagerUpdateContextValue {
  const value = useContext(ManagerUpdateContext);
  if (!value) throw new Error("useManagerUpdate must be used inside ManagerUpdateProvider");
  return value;
}

export function ManagerUpdateBanner() {
  const { t } = useI18n();
  const state = useManagerUpdate();

  if (state.completion) {
    return (
      <StatusBanner
        tone="ok"
        action={
          <button className="linkbtn" onClick={state.dismissCompletion}>
            {t("nav.close")}
          </button>
        }
      >
        {t("managerUpdate.completed", { version: state.completion.to })}
      </StatusBanner>
    );
  }
  if (
    state.update &&
    (state.status === "installed-awaiting-relaunch" || state.status === "relaunching")
  ) {
    return (
      <StatusBanner
        tone="warn"
        action={
          <button
            className="linkbtn"
            disabled={state.status === "relaunching"}
            onClick={() => void state.retryRelaunch()}
          >
            {t("managerUpdate.restart")}
          </button>
        }
      >
        {t("managerUpdate.restartRequired", { version: state.update.version })}
      </StatusBanner>
    );
  }
  if (!state.update || state.snoozed) return null;

  return (
    <div className="manager-update-banner">
      <StatusBanner
        tone="info"
        icon="arrowUp"
        action={
          <span className="manager-update-banner-actions">
            <button className="linkbtn" onClick={state.openDetails}>
              {t("managerUpdate.viewNotes")}
            </button>
            <button className="linkbtn" onClick={state.remindLater}>
              {t("managerUpdate.later")}
            </button>
          </span>
        }
      >
        {t("about.mgrFound", { version: state.update.version })}
      </StatusBanner>
    </div>
  );
}

function progressLabel(progress: ManagerUpdateProgress | null): string | null {
  if (!progress || progress.phase !== "downloading") return null;
  const downloaded = (progress.downloaded / 1024 / 1024).toFixed(1);
  if (!progress.total || progress.total <= 0) return `${downloaded} MiB`;
  const total = (progress.total / 1024 / 1024).toFixed(1);
  return `${downloaded} / ${total} MiB`;
}

export function ManagerUpdateSheet({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { t } = useI18n();
  const state = useManagerUpdate();
  const titleId = useId();
  const bodyId = useId();
  const busy = ["checking", "downloading", "installing", "relaunching"].includes(
    state.status,
  );
  const noUpdateResult = state.status === "up-to-date" || state.status === "development";
  const busyLabel =
    state.status === "checking"
      ? t("about.mgrChecking")
      : state.status === "downloading"
        ? t("managerUpdate.downloading")
        : t("progress.installing");
  const pct =
    state.progress?.total && state.progress.total > 0
      ? Math.min(100, (state.progress.downloaded / state.progress.total) * 100)
      : null;
  const notes = state.update?.body?.trim();
  const awaitingRelaunch = state.status === "installed-awaiting-relaunch";
  const networkFailure =
    state.failure?.code === "network" || state.failure?.code === "timeout";

  return (
    <Sheet
      open={state.detailsOpen}
      onDismiss={state.closeDetails}
      dismissable={!busy}
      labelledBy={titleId}
      describedBy={bodyId}
      initialFocus={state.failure ? "first" : "primary"}
    >
      <Ring
        icon={state.failure ? "alert" : busy ? "loader" : "arrowUp"}
        spin={busy}
        variant={state.failure ? "amber" : "accent"}
      />
      <h3 id={titleId}>
        {state.status === "checking"
          ? t("about.checkManager")
          : state.update
            ? t("confirm.title", { version: state.update.version })
            : t("about.checkManager")}
      </h3>
      <p id={bodyId}>
        {state.status === "checking"
          ? t("about.mgrChecking")
          : awaitingRelaunch && state.update
            ? t("managerUpdate.restartRequired", { version: state.update.version })
            : state.status === "up-to-date"
              ? t("about.mgrUpToDate")
              : state.status === "development"
                ? t("about.mgrDev")
                : state.update
                  ? t("about.mgrConfirmBody")
                  : t("about.mgrUnavailable")}
      </p>

      {state.update ? (
        <div
          className="manager-update-version"
          aria-label={`${state.update.currentVersion} → ${state.update.version}`}
        >
          <span>{state.update.currentVersion}</span>
          <span aria-hidden="true">→</span>
          <span>{state.update.version}</span>
        </div>
      ) : null}

      {notes ? (
        <section className="manager-update-notes">
          <strong>{t("managerUpdate.notes")}</strong>
          <pre>{notes}</pre>
        </section>
      ) : state.update ? (
        <p className="manager-update-empty-notes">{t("managerUpdate.noNotes")}</p>
      ) : null}

      {busy ? (
        <div className="manager-update-progress" aria-live="polite">
          <div className="manager-update-progress-head">
            <span>{busyLabel}</span>
            <span>{progressLabel(state.progress)}</span>
          </div>
          <div className="bar">
            <div
              className={`bar-fill${pct == null ? " indeterminate" : ""}`}
              style={pct == null ? undefined : { width: `${pct}%` }}
            />
          </div>
        </div>
      ) : null}

      {state.failure ? <FailureBanner failure={state.failure} /> : null}
      {networkFailure ? (
        <p className="manager-update-network-hint">{t("managerUpdate.networkHint")}</p>
      ) : null}

      <div className="row2 sheet-actions manager-update-sheet-actions">
        {state.status === "installed-awaiting-relaunch" ? (
          <>
            <button className="btn ghost" onClick={state.closeDetails}>
              {t("confirm.cancel")}
            </button>
            <button className="btn primary" onClick={() => void state.retryRelaunch()}>
              {t("managerUpdate.restart")}
            </button>
          </>
        ) : state.failure ? (
          <>
            {networkFailure ? (
              <button
                className="btn ghost"
                onClick={() => {
                  state.closeDetails();
                  onOpenSettings();
                }}
              >
                {t("settings.network.header")}
              </button>
            ) : (
              <button className="btn ghost" onClick={state.closeDetails}>
                {t("nav.close")}
              </button>
            )}
            <button
              className="btn primary"
              onClick={() =>
                void (state.update
                  ? state.install()
                  : state.check({ manual: true, openWhenAvailable: true }))
              }
            >
              {t("settings.retry")}
            </button>
          </>
        ) : busy ? (
          <button className="btn primary" disabled aria-busy="true">
            <Icon name="loader" className="spinicon" />
            {busyLabel}
          </button>
        ) : noUpdateResult ? (
          <button className="btn primary" onClick={state.closeDetails}>
            {t("nav.close")}
          </button>
        ) : (
          <>
            <button className="btn ghost" onClick={state.remindLater}>
              {t("managerUpdate.later")}
            </button>
            <button
              className="btn primary"
              onClick={() => void state.install()}
              disabled={!state.update}
            >
              {t("confirm.ok")}
            </button>
          </>
        )}
      </div>
    </Sheet>
  );
}
