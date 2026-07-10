import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import type { OperationSnapshot } from "../../shared/types";
import { useOperationReattach } from "./useOperationReattach";

vi.mock("../../services/managerApi", () => ({
  managerApi: {
    getOperationSnapshot: vi.fn(),
  },
}));

const getSnapshot = vi.mocked(managerApi.getOperationSnapshot);

const ACTIVE: OperationSnapshot = {
  id: "op-42",
  kind: "update",
  phase: "downloading",
  progress: { downloaded: 512, total: 2048, source: "cdn.test", operationId: "op-42" },
  paused: false,
  cancellable: true,
  interruptible: true,
};

describe("useOperationReattach", () => {
  beforeEach(() => {
    getSnapshot.mockReset();
  });

  it("restores busy/progress and rebuilds the listener from a backend snapshot", async () => {
    getSnapshot.mockResolvedValue(ACTIVE);
    const startDlListen = vi.fn(async () => () => {});
    const applySnapshotProgress = vi.fn();
    const resetStop = vi.fn();
    const setBusy = vi.fn();
    const setPaused = vi.fn();
    const onOperationEnded = vi.fn();

    renderHook(() =>
      useOperationReattach({
        startDlListen,
        applySnapshotProgress,
        resetStop,
        setBusy,
        setPaused,
        onOperationEnded,
        isLocallyBusy: () => false,
      }),
    );

    await waitFor(() => {
      expect(setBusy).toHaveBeenCalledWith("perform");
    });
    expect(applySnapshotProgress).toHaveBeenCalledWith(ACTIVE.progress);
    expect(setPaused).toHaveBeenCalledWith(null);
    expect(startDlListen).toHaveBeenCalledWith({
      operationId: "op-42",
      preserveProgress: true,
    });
  });

  it("restores a paused download screen from snapshot", async () => {
    getSnapshot.mockResolvedValue({
      ...ACTIVE,
      kind: "install",
      paused: true,
      progress: { downloaded: 10, total: 100, source: "x" },
    });
    const setBusy = vi.fn();
    const setPaused = vi.fn();

    renderHook(() =>
      useOperationReattach({
        startDlListen: vi.fn(async () => () => {}),
        applySnapshotProgress: vi.fn(),
        resetStop: vi.fn(),
        setBusy,
        setPaused,
        onOperationEnded: vi.fn(),
        isLocallyBusy: () => false,
      }),
    );

    await waitFor(() => {
      expect(setBusy).toHaveBeenCalledWith("install");
    });
    expect(setPaused).toHaveBeenCalledWith({
      kind: "install",
      dl: { downloaded: 10, total: 100, source: "x" },
    });
  });

  it("polls until the lease ends then clears UI and notifies", async () => {
    getSnapshot
      .mockResolvedValueOnce(ACTIVE) // mount query
      .mockResolvedValueOnce(ACTIVE) // first poll still busy
      .mockResolvedValueOnce(null); // second poll: finished

    const resetStop = vi.fn();
    const setBusy = vi.fn();
    const setPaused = vi.fn();
    const onOperationEnded = vi.fn();

    renderHook(() =>
      useOperationReattach({
        startDlListen: vi.fn(async () => () => {}),
        applySnapshotProgress: vi.fn(),
        resetStop,
        setBusy,
        setPaused,
        onOperationEnded,
        isLocallyBusy: () => false,
      }),
    );

    await waitFor(() => expect(setBusy).toHaveBeenCalledWith("perform"));

    // First poll tick (800ms).
    await act(async () => {
      await new Promise((r) => setTimeout(r, 850));
    });
    // Second poll tick ends the op.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 850));
    });

    await waitFor(() => {
      expect(onOperationEnded).toHaveBeenCalled();
    });
    expect(resetStop).toHaveBeenCalled();
    expect(setBusy).toHaveBeenCalledWith(null);
    expect(setPaused).toHaveBeenCalledWith(null);
  });

  it("does not attach when a local perform/install already owns the UI", async () => {
    getSnapshot.mockResolvedValue(ACTIVE);
    const startDlListen = vi.fn(async () => () => {});
    const setBusy = vi.fn();

    renderHook(() =>
      useOperationReattach({
        startDlListen,
        applySnapshotProgress: vi.fn(),
        resetStop: vi.fn(),
        setBusy,
        setPaused: vi.fn(),
        onOperationEnded: vi.fn(),
        isLocallyBusy: () => true,
      }),
    );

    // Give the async mount path a chance to run.
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(startDlListen).not.toHaveBeenCalled();
    expect(setBusy).not.toHaveBeenCalled();
  });
});
