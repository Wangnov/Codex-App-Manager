import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { listen } from "@tauri-apps/api/event";

import type { DownloadProgress } from "../../shared/types";
import { I18nProvider } from "../i18n";
import { useDownloadProgress } from "./useDownloadProgress";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

const listenMock = vi.mocked(listen);

function wrapper({ children }: { children: React.ReactNode }) {
  return <I18nProvider>{children}</I18nProvider>;
}

describe("useDownloadProgress", () => {
  let onProgress: ((event: { payload: DownloadProgress }) => void) | undefined;

  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
    onProgress = undefined;
    listenMock.mockReset();
    listenMock.mockImplementation(async (_event, handler) => {
      onProgress = handler as (event: { payload: DownloadProgress }) => void;
      return () => {
        onProgress = undefined;
      };
    });
  });

  it("latches the first operation id and rejects late events from older ops", async () => {
    const { result } = renderHook(
      () =>
        useDownloadProgress({
          eventName: "mac://download-progress",
          pauseDownload: vi.fn(async () => true),
          cancelDownload: vi.fn(async () => true),
          cannotCancelMessage: "cannot cancel",
          onError: vi.fn(),
        }),
      { wrapper },
    );

    await act(async () => {
      await result.current.startDlListen();
    });

    act(() => {
      onProgress?.({
        payload: {
          downloaded: 10,
          total: 100,
          source: "a.test",
          operationId: "op-new",
        },
      });
    });
    expect(result.current.dl?.downloaded).toBe(10);
    expect(result.current.activeOperationIdRef.current).toBe("op-new");

    // Late event from a previous operation must not overwrite live progress.
    act(() => {
      onProgress?.({
        payload: {
          downloaded: 999,
          total: 100,
          source: "old.test",
          operationId: "op-old",
        },
      });
    });
    expect(result.current.dl?.downloaded).toBe(10);

    // Same op continues to update.
    act(() => {
      onProgress?.({
        payload: {
          downloaded: 40,
          total: 100,
          source: "a.test",
          operationId: "op-new",
        },
      });
    });
    expect(result.current.dl?.downloaded).toBe(40);
  });

  it("rebuilds a listener bound to a reattached operation id", async () => {
    const { result } = renderHook(
      () =>
        useDownloadProgress({
          eventName: "win://download-progress",
          pauseDownload: vi.fn(async () => true),
          cancelDownload: vi.fn(async () => true),
          cannotCancelMessage: "cannot cancel",
          onError: vi.fn(),
        }),
      { wrapper },
    );

    result.current.applySnapshotProgress({
      downloaded: 25,
      total: 200,
      source: "mirror.test",
      operationId: "reattach-1",
    });

    await act(async () => {
      await result.current.startDlListen({
        operationId: "reattach-1",
        preserveProgress: true,
      });
    });

    expect(result.current.dl?.downloaded).toBe(25);
    expect(result.current.activeOperationIdRef.current).toBe("reattach-1");

    // Foreign op rejected.
    act(() => {
      onProgress?.({
        payload: {
          downloaded: 1,
          total: 200,
          source: "x",
          operationId: "other",
        },
      });
    });
    expect(result.current.dl?.downloaded).toBe(25);

    // Matching op accepted.
    act(() => {
      onProgress?.({
        payload: {
          downloaded: 80,
          total: 200,
          source: "mirror.test",
          operationId: "reattach-1",
        },
      });
    });
    expect(result.current.dl?.downloaded).toBe(80);

    // Untagged events rejected while bound to a concrete id.
    act(() => {
      onProgress?.({
        payload: {
          downloaded: 5,
          total: 200,
          source: "x",
        },
      });
    });
    expect(result.current.dl?.downloaded).toBe(80);
  });

  it("resetStop clears the active operation id", async () => {
    const { result } = renderHook(
      () =>
        useDownloadProgress({
          eventName: "mac://download-progress",
          pauseDownload: vi.fn(async () => true),
          cancelDownload: vi.fn(async () => true),
          cannotCancelMessage: "cannot cancel",
          onError: vi.fn(),
        }),
      { wrapper },
    );

    await act(async () => {
      await result.current.startDlListen({ operationId: "x" });
    });
    expect(result.current.activeOperationIdRef.current).toBe("x");

    act(() => {
      result.current.resetStop();
    });
    expect(result.current.activeOperationIdRef.current).toBeNull();
    expect(result.current.dl).toBeNull();
  });

  it("startDlListen registers the platform event channel", async () => {
    const { result } = renderHook(
      () =>
        useDownloadProgress({
          eventName: "mac://download-progress",
          pauseDownload: vi.fn(async () => true),
          cancelDownload: vi.fn(async () => true),
          cannotCancelMessage: "cannot cancel",
          onError: vi.fn(),
        }),
      { wrapper },
    );

    await act(async () => {
      await result.current.startDlListen();
    });

    await waitFor(() => {
      expect(listenMock).toHaveBeenCalledWith("mac://download-progress", expect.any(Function));
    });
  });
});
