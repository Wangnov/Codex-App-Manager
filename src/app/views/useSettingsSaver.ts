import { useCallback, useRef, useState } from "react";

import { errorMessage, managerApi } from "../../services/managerApi";
import type { AppSettings } from "../../shared/types";

export type SaveStatus = "idle" | "saving" | "error";

export function useSettingsSaver(initial: AppSettings) {
  const [value, setValue] = useState<AppSettings>(initial);
  const [status, setStatus] = useState<SaveStatus>("idle");
  const [error, setError] = useState<string | null>(null);

  const seqRef = useRef(0);
  const inFlightRef = useRef(false);
  const pendingRef = useRef<AppSettings | null>(null);
  const lastValueRef = useRef(initial);

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
    const mySeq = ++seqRef.current;
    setStatus("saving");
    setError(null);

    try {
      const saved = await managerApi.setSettings(next);
      if (mySeq === seqRef.current && pendingRef.current == null) {
        lastValueRef.current = saved;
        setValue(saved);
        setStatus("idle");
      }
    } catch (cause) {
      if (mySeq === seqRef.current && pendingRef.current == null) {
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
      setValue(next);
      pendingRef.current = next;
      void flush();
    },
    [flush],
  );

  const retry = useCallback(() => {
    pendingRef.current = pendingRef.current ?? value;
    void flush();
  }, [flush, value]);

  const reset = useCallback((settings: AppSettings) => {
    lastValueRef.current = settings;
    pendingRef.current = null;
    setValue(settings);
    setStatus("idle");
    setError(null);
  }, []);

  return {
    settings: value,
    status,
    error,
    update,
    retry,
    reset,
    setDraft: setValue,
  };
}
