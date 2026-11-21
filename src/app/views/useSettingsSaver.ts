import { useCallback, useRef, useState } from "react";

import { errorMessage, managerApi } from "../../services/managerApi";
import type { AppSettings } from "../../shared/types";

export type SaveStatus = "idle" | "saving" | "error";

/** Incomplete custom modes stay as UI drafts until a value is present. */
export function settingsPayloadForSave(
  next: AppSettings,
  lastSaved: AppSettings,
): AppSettings {
  const payload = { ...next };
  if (payload.source === "custom" && !payload.customUrl.trim()) {
    payload.source = lastSaved.source;
    payload.customUrl = lastSaved.customUrl;
  }
  if (payload.proxyMode === "custom" && !payload.customProxyUrl.trim()) {
    payload.proxyMode = lastSaved.proxyMode;
    payload.customProxyUrl = lastSaved.customProxyUrl;
  }
  return payload;
}

/** Keep incomplete custom drafts visible after a successful save of other fields. */
export function mergeSavedKeepingCustomDraft(
  draft: AppSettings,
  saved: AppSettings,
): AppSettings {
  const keepSource = draft.source === "custom" && !draft.customUrl.trim();
  const keepProxy = draft.proxyMode === "custom" && !draft.customProxyUrl.trim();
  return {
    ...saved,
    source: keepSource ? draft.source : saved.source,
    customUrl: keepSource ? draft.customUrl : saved.customUrl,
    proxyMode: keepProxy ? draft.proxyMode : saved.proxyMode,
    customProxyUrl: keepProxy ? draft.customProxyUrl : saved.customProxyUrl,
  };
}

export function useSettingsSaver(initial: AppSettings) {
  const [value, setValue] = useState<AppSettings>(initial);
  const [savedSettings, setSavedSettings] = useState<AppSettings>(initial);
  const [status, setStatus] = useState<SaveStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [hydrated, setHydrated] = useState(false);

  const inFlightRef = useRef(false);
  const pendingRef = useRef<AppSettings | null>(null);
  const lastValueRef = useRef(initial);
  /** True once the user has drafted or committed a change this session. */
  const dirtyRef = useRef(false);

  const flush = useCallback(async () => {
    if (inFlightRef.current) {
      return;
    }
    const next = pendingRef.current;
    if (next == null) {
      return;
    }

    pendingRef.current = null;
    inFlightRef.current = true;
    setStatus("saving");
    setError(null);

    try {
      const saved = await managerApi.setSettings(settingsPayloadForSave(next, lastValueRef.current));
      lastValueRef.current = saved;
      setSavedSettings(saved);
      if (pendingRef.current == null) {
        // A response only owns the draft it submitted. Typing into a custom
        // field does not enqueue a write until blur, but still supersedes it.
        setValue((prev) => prev === next ? mergeSavedKeepingCustomDraft(prev, saved) : prev);
        setStatus("idle");
      }
    } catch (cause) {
      if (pendingRef.current == null) {
        setStatus("error");
        setError(errorMessage(cause));
      }
    } finally {
      inFlightRef.current = false;
      if (pendingRef.current != null) {
        void flush();
      }
    }
  }, []);

  const update = useCallback(
    (next: AppSettings) => {
      dirtyRef.current = true;
      setValue(next);
      pendingRef.current = next;
      void flush();
    },
    [flush],
  );

  const setDraft = useCallback((next: AppSettings) => {
    dirtyRef.current = true;
    setValue(next);
  }, []);

  const retry = useCallback(() => {
    dirtyRef.current = true;
    pendingRef.current = pendingRef.current ?? value;
    void flush();
  }, [flush, value]);

  /**
   * Apply server/local settings from the initial load. Never overwrites user
   * edits or in-flight writes (slow IPC race with early interaction).
   */
  const hydrate = useCallback((settings: AppSettings) => {
    if (dirtyRef.current || pendingRef.current != null || inFlightRef.current) {
      setHydrated(true);
      return;
    }
    lastValueRef.current = settings;
    setSavedSettings(settings);
    setValue(settings);
    setStatus("idle");
    setError(null);
    setHydrated(true);
  }, []);

  /** Force-replace local state (install-root commands, etc.) after a known write. */
  const reset = useCallback((settings: AppSettings) => {
    dirtyRef.current = false;
    lastValueRef.current = settings;
    setSavedSettings(settings);
    pendingRef.current = null;
    setValue(settings);
    setStatus("idle");
    setError(null);
    setHydrated(true);
  }, []);

  return {
    settings: value,
    savedSettings,
    status,
    error,
    hydrated,
    update,
    retry,
    hydrate,
    reset,
    setDraft,
  };
}
