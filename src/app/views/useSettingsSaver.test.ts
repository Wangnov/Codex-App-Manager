import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import { DEFAULT_SETTINGS, type AppSettings } from "../../shared/types";
import { useSettingsSaver } from "./useSettingsSaver";

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
});
