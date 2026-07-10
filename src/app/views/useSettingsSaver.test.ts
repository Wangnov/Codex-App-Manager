import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import { DEFAULT_SETTINGS, type AppSettings } from "../../shared/types";
import type { TKey } from "../i18n";
import { useSettingsSaver } from "./useSettingsSaver";

vi.mock("../../services/managerApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      ...actual.managerApi,
      setSettings: vi.fn(),
    },
  };
});

const setSettings = vi.mocked(managerApi.setSettings);
const t = (key: TKey) => (key === "error.generic" ? "localized-generic" : key);

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
    const { result } = renderHook(() => useSettingsSaver(DEFAULT_SETTINGS, t));
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

  it("surfaces localized failures and retries the latest value", async () => {
    setSettings
      .mockRejectedValueOnce(new Error("disk full"))
      .mockResolvedValueOnce({ ...DEFAULT_SETTINGS, askBefore: false });
    const { result } = renderHook(() => useSettingsSaver(DEFAULT_SETTINGS, t));
    const next = { ...DEFAULT_SETTINGS, askBefore: false };

    act(() => result.current.update(next));

    await waitFor(() => expect(result.current.status).toBe("error"));
    // Raw engine text must not leak into the save banner.
    expect(result.current.error).toBe("localized-generic");
    expect(result.current.error).not.toContain("disk full");

    act(() => result.current.retry());

    await waitFor(() => expect(result.current.status).toBe("idle"));
    expect(setSettings).toHaveBeenCalledTimes(2);
    expect(result.current.settings.askBefore).toBe(false);
  });
});
