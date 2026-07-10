import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import { DEFAULT_SETTINGS, type AppSettings } from "../../shared/types";
import {
  mergeSavedKeepingCustomDraft,
  settingsPayloadForSave,
  useSettingsSaver,
} from "./useSettingsSaver";

vi.mock("../../services/managerApi", () => ({
  errorMessage: (cause: unknown) => (cause instanceof Error ? cause.message : String(cause)),
  managerApi: {
    setSettings: vi.fn(),
  },
}));

const setSettings = vi.mocked(managerApi.setSettings);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("settingsPayloadForSave", () => {
  it("keeps incomplete custom modes on the last saved values", () => {
    const last = {
      ...DEFAULT_SETTINGS,
      source: "mirror" as const,
      proxyMode: "direct" as const,
    };
    const draft = {
      ...DEFAULT_SETTINGS,
      source: "custom" as const,
      customUrl: "  ",
      proxyMode: "custom" as const,
      customProxyUrl: "",
      askBefore: false,
    };
    expect(settingsPayloadForSave(draft, last)).toEqual({
      ...draft,
      source: "mirror",
      customUrl: last.customUrl,
      proxyMode: "direct",
      customProxyUrl: last.customProxyUrl,
    });
  });

  it("passes through complete custom modes", () => {
    const last = { ...DEFAULT_SETTINGS };
    const next = {
      ...DEFAULT_SETTINGS,
      source: "custom" as const,
      customUrl: "https://example.test/feed",
      proxyMode: "custom" as const,
      customProxyUrl: "socks5h://127.0.0.1:7890",
    };
    expect(settingsPayloadForSave(next, last)).toEqual(next);
  });
});

describe("mergeSavedKeepingCustomDraft", () => {
  it("preserves incomplete custom drafts after other fields save", () => {
    const draft = {
      ...DEFAULT_SETTINGS,
      source: "custom" as const,
      customUrl: "",
      proxyMode: "custom" as const,
      customProxyUrl: "",
      askBefore: false,
    };
    const saved = {
      ...DEFAULT_SETTINGS,
      source: "mirror" as const,
      proxyMode: "direct" as const,
      askBefore: false,
    };
    expect(mergeSavedKeepingCustomDraft(draft, saved)).toEqual({
      ...saved,
      source: "custom",
      customUrl: "",
      proxyMode: "custom",
      customProxyUrl: "",
    });
  });
});

describe("useSettingsSaver", () => {
  beforeEach(() => {
    setSettings.mockReset();
  });

  it("serializes writes and does not let an older response overwrite the latest draft", async () => {
    const first = deferred<AppSettings>();
    const second = deferred<AppSettings>();
    setSettings.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { result } = renderHook(() => useSettingsSaver(DEFAULT_SETTINGS));
    const older = { ...DEFAULT_SETTINGS, source: "mirror" as const };
    const latest = { ...DEFAULT_SETTINGS, source: "custom" as const, customUrl: "https://x.test" };

    act(() => result.current.update(older));
    act(() => result.current.update(latest));

    expect(setSettings).toHaveBeenCalledTimes(1);
    expect(result.current.settings).toEqual(latest);
    expect(result.current.status).toBe("saving");

    act(() => first.resolve(older));
    await waitFor(() => expect(setSettings).toHaveBeenCalledTimes(2));
    expect(result.current.settings).toEqual(latest);

    act(() => second.resolve(latest));
    await waitFor(() => expect(result.current.status).toBe("idle"));
    expect(result.current.settings).toEqual(latest);
  });

  it("surfaces failures and retries the latest value", async () => {
    setSettings
      .mockRejectedValueOnce(new Error("disk full"))
      .mockResolvedValueOnce({ ...DEFAULT_SETTINGS, askBefore: false });
    const { result } = renderHook(() => useSettingsSaver(DEFAULT_SETTINGS));
    const next = { ...DEFAULT_SETTINGS, askBefore: false };

    act(() => result.current.update(next));

    await waitFor(() => expect(result.current.status).toBe("error"));
    expect(result.current.error).toBe("disk full");

    act(() => result.current.retry());

    await waitFor(() => expect(result.current.status).toBe("idle"));
    expect(setSettings).toHaveBeenCalledTimes(2);
    expect(result.current.settings.askBefore).toBe(false);
  });

  it("does not let a slow hydrate overwrite user edits", async () => {
    const { result } = renderHook(() => useSettingsSaver(DEFAULT_SETTINGS));
    const edited = { ...DEFAULT_SETTINGS, source: "mirror" as const };
    setSettings.mockResolvedValue(edited);

    act(() => result.current.update(edited));
    act(() =>
      result.current.hydrate({
        ...DEFAULT_SETTINGS,
        source: "official",
        checkOnStartup: false,
      }),
    );

    expect(result.current.settings.source).toBe("mirror");
    expect(result.current.hydrated).toBe(true);
    await waitFor(() => expect(result.current.status).toBe("idle"));
  });

  it("hydrates when the form is still clean", () => {
    const { result } = renderHook(() => useSettingsSaver(DEFAULT_SETTINGS));
    const loaded = { ...DEFAULT_SETTINGS, source: "mirror" as const, periodicCheck: false };

    act(() => result.current.hydrate(loaded));

    expect(result.current.settings).toEqual(loaded);
    expect(result.current.hydrated).toBe(true);
  });

  it("saves other fields without persisting an incomplete custom draft", async () => {
    setSettings.mockImplementation(async (next) => next);
    const { result } = renderHook(() => useSettingsSaver(DEFAULT_SETTINGS));

    act(() =>
      result.current.setDraft({
        ...DEFAULT_SETTINGS,
        source: "custom",
        customUrl: "",
      }),
    );
    act(() =>
      result.current.update({
        ...DEFAULT_SETTINGS,
        source: "custom",
        customUrl: "",
        askBefore: false,
      }),
    );

    await waitFor(() => expect(setSettings).toHaveBeenCalledTimes(1));
    expect(setSettings).toHaveBeenCalledWith({
      ...DEFAULT_SETTINGS,
      source: "auto",
      customUrl: "",
      askBefore: false,
    });
    // UI keeps the custom draft selection.
    expect(result.current.settings.source).toBe("custom");
    expect(result.current.settings.askBefore).toBe(false);
  });

  it("persists an explicit auto coerce when clearing a saved custom source", async () => {
    setSettings.mockImplementation(async (next) => next);
    const initial = {
      ...DEFAULT_SETTINGS,
      source: "custom" as const,
      customUrl: "https://example.test/feed",
    };
    const { result } = renderHook(() => useSettingsSaver(initial));
    act(() => result.current.hydrate(initial));

    act(() =>
      result.current.update({
        ...initial,
        source: "auto",
        customUrl: "",
      }),
    );

    await waitFor(() => expect(setSettings).toHaveBeenCalledTimes(1));
    expect(setSettings).toHaveBeenCalledWith({
      ...initial,
      source: "auto",
      customUrl: "",
    });
    expect(result.current.settings.source).toBe("auto");
    expect(result.current.settings.customUrl).toBe("");
  });

  it("persists an explicit system coerce when clearing a saved custom proxy", async () => {
    setSettings.mockImplementation(async (next) => next);
    const initial = {
      ...DEFAULT_SETTINGS,
      proxyMode: "custom" as const,
      customProxyUrl: "socks5h://127.0.0.1:7890",
    };
    const { result } = renderHook(() => useSettingsSaver(initial));
    act(() => result.current.hydrate(initial));

    act(() =>
      result.current.update({
        ...initial,
        proxyMode: "system",
        customProxyUrl: "",
      }),
    );

    await waitFor(() => expect(setSettings).toHaveBeenCalledTimes(1));
    expect(setSettings).toHaveBeenCalledWith({
      ...initial,
      proxyMode: "system",
      customProxyUrl: "",
    });
    expect(result.current.settings.proxyMode).toBe("system");
    expect(result.current.settings.customProxyUrl).toBe("");
  });
});
