import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import type { OperationSnapshot } from "../../shared/types";
import { useOperationReattach } from "./useOperationReattach";

vi.mock("../../services/managerApi", () => ({
  managerApi: {
    getOperationSnapshot: vi.fn(),
    getPausedOperationSnapshot: vi.fn(),
    getOperationCompletion: vi.fn(),
  },
}));

const getSnapshot = vi.mocked(managerApi.getOperationSnapshot);
const getPausedSnapshot = vi.mocked(managerApi.getPausedOperationSnapshot);
const getCompletion = vi.mocked(managerApi.getOperationCompletion);

const ACTIVE: OperationSnapshot = {
  id: "op-42",
  kind: "update",
  phase: "downloading",
  progress: {
    downloaded: 512,
    total: 2048,
    source: "cdn.test",
    operationId: "op-42",
  },
  paused: false,
  cancellable: true,
  interruptible: true,
};

describe("useOperationReattach", () => {
  beforeEach(() => {
    getSnapshot.mockReset();
    getPausedSnapshot.mockReset();
    getCompletion.mockReset();
    getPausedSnapshot.mockResolvedValue(null);
    getCompletion.mockResolvedValue(null);
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

  it("keeps the exact historical target when a reattached pause ends its lease", async () => {
    const historical = {
      selection: {
        releaseTag: "codex-app-26.623.101652",
        version: "26.623.101652",
        assetName: "Codex-mac-arm64.dmg",
        architecture: "arm64" as const,
        format: "dmg" as const,
        packageVersion: null,
        localPath: null,
        localFileName: null,
      },
      blockUpdates: true,
      expectation: {
        platform: "macos" as const,
        currentPath: "/Applications/Codex.app",
        currentBuild: 5813,
      },
    };
    const paused = {
      ...ACTIVE,
      paused: true,
      historical,
    };
    getSnapshot.mockResolvedValueOnce(paused).mockResolvedValueOnce(null);
    const setPaused = vi.fn();
    const onOperationEnded = vi.fn();

    renderHook(() =>
      useOperationReattach({
        startDlListen: vi.fn(async () => () => {}),
        applySnapshotProgress: vi.fn(),
        resetStop: vi.fn(),
        setBusy: vi.fn(),
        setPaused,
        onOperationEnded,
        isLocallyBusy: () => false,
      }),
    );

    await waitFor(() => {
      expect(setPaused).toHaveBeenCalledWith({
        kind: "perform",
        dl: ACTIVE.progress,
        historical,
      });
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 850));
    });
    await waitFor(() => expect(onOperationEnded).toHaveBeenCalled());
    expect(setPaused).not.toHaveBeenCalledWith(null);
  });

  it("recovers a pause that completed between active-snapshot polls", async () => {
    const terminal = {
      ...ACTIVE,
      paused: true,
      historical: {
        selection: {
          releaseTag: "codex-app-0.9.0",
          version: "0.9.0",
          assetName: "OpenAI.Codex_0.9.0.0_x64.msix",
          architecture: "x64" as const,
          format: "msix" as const,
          packageVersion: "0.9.0.0",
          localPath: null,
          localFileName: null,
        },
        blockUpdates: true,
        expectation: {
          platform: "windows" as const,
          currentPath: null,
          currentVersion: null,
          currentSource: null,
        },
        installRoot: "C:\\Codex",
      },
    };
    getSnapshot.mockResolvedValueOnce(ACTIVE).mockResolvedValueOnce(null);
    getPausedSnapshot.mockResolvedValue(terminal);
    const setPaused = vi.fn();

    renderHook(() =>
      useOperationReattach({
        startDlListen: vi.fn(async () => () => {}),
        applySnapshotProgress: vi.fn(),
        resetStop: vi.fn(),
        setBusy: vi.fn(),
        setPaused,
        onOperationEnded: vi.fn(),
        isLocallyBusy: () => false,
      }),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 850));
    });
    await waitFor(() =>
      expect(setPaused).toHaveBeenCalledWith({
        kind: "perform",
        dl: ACTIVE.progress,
        historical: terminal.historical,
      }),
    );
    expect(setPaused).not.toHaveBeenLastCalledWith(null);
  });

  it.each(["failed-before-commit", "rolled-back"] as const)(
    "reconnects an older historical pause after a safe %s resume outcome",
    async (state) => {
      const historical = {
        selection: {
          releaseTag: "codex-app-0.9.0",
          version: "0.9.0",
          assetName: "OpenAI.Codex_0.9.0.0_x64.msix",
          architecture: "x64" as const,
          format: "msix" as const,
          packageVersion: "0.9.0.0",
          localPath: null,
          localFileName: null,
        },
        blockUpdates: true,
        expectation: {
          platform: "windows" as const,
          currentPath: null,
          currentVersion: "1.0.0.0",
          currentSource: "msix" as const,
        },
        installRoot: "C:\\Codex",
      };
      const resumed = {
        ...ACTIVE,
        id: "resume-op",
        progress: { ...ACTIVE.progress!, operationId: "resume-op" },
      };
      const retained = {
        ...ACTIVE,
        id: "old-pause",
        paused: true,
        historical,
      };
      getSnapshot.mockResolvedValueOnce(resumed).mockResolvedValueOnce(null);
      getPausedSnapshot.mockResolvedValue(retained);
      getCompletion.mockResolvedValue({
        id: resumed.id,
        kind: resumed.kind,
        phase: "verifying",
        state,
      });
      const setPaused = vi.fn();
      const onOperationEnded = vi.fn();

      renderHook(() =>
        useOperationReattach({
          startDlListen: vi.fn(async () => () => {}),
          applySnapshotProgress: vi.fn(),
          resetStop: vi.fn(),
          setBusy: vi.fn(),
          setPaused,
          onOperationEnded,
          isLocallyBusy: () => false,
        }),
      );

      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 850));
      });
      await waitFor(() => expect(onOperationEnded).toHaveBeenCalled());
      expect(getCompletion).toHaveBeenCalledWith("resume-op");
      expect(setPaused).toHaveBeenLastCalledWith({
        kind: "perform",
        dl: retained.progress,
        historical,
      });
    },
  );

  it("rejects an older historical pause after an outcome-unknown resume", async () => {
    const historical = {
      selection: {
        releaseTag: "codex-app-0.9.0",
        version: "0.9.0",
        assetName: "OpenAI.Codex_0.9.0.0_x64.msix",
        architecture: "x64" as const,
        format: "msix" as const,
        packageVersion: "0.9.0.0",
        localPath: null,
        localFileName: null,
      },
      blockUpdates: true,
      expectation: {
        platform: "windows" as const,
        currentPath: null,
        currentVersion: "1.0.0.0",
        currentSource: "msix" as const,
      },
      installRoot: "C:\\Codex",
    };
    const resumed = { ...ACTIVE, id: "resume-op" };
    getSnapshot.mockResolvedValueOnce(resumed).mockResolvedValueOnce(null);
    getPausedSnapshot.mockResolvedValue({
      ...ACTIVE,
      id: "old-pause",
      paused: true,
      historical,
    });
    getCompletion.mockResolvedValue({
      id: resumed.id,
      kind: resumed.kind,
      phase: "committing",
      state: "outcome-unknown",
    });
    const setPaused = vi.fn();
    const onOperationEnded = vi.fn();

    renderHook(() =>
      useOperationReattach({
        startDlListen: vi.fn(async () => () => {}),
        applySnapshotProgress: vi.fn(),
        resetStop: vi.fn(),
        setBusy: vi.fn(),
        setPaused,
        onOperationEnded,
        isLocallyBusy: () => false,
      }),
    );

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 850));
    });
    await waitFor(() => expect(onOperationEnded).toHaveBeenCalled());
    expect(getCompletion).toHaveBeenCalledWith("resume-op");
    expect(setPaused).not.toHaveBeenCalledWith(
      expect.objectContaining({ historical }),
    );
    expect(setPaused).toHaveBeenLastCalledWith(null);
  });

  it("restores a terminal pause when the active lease already ended before mount", async () => {
    const terminal: OperationSnapshot = {
      ...ACTIVE,
      kind: "install",
      paused: true,
    };
    getSnapshot.mockResolvedValue(null);
    getPausedSnapshot.mockResolvedValue(terminal);
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

    await waitFor(() =>
      expect(setPaused).toHaveBeenCalledWith({
        kind: "install",
        dl: ACTIVE.progress,
      }),
    );
    expect(setBusy).toHaveBeenCalledWith("install");
    expect(setBusy).toHaveBeenLastCalledWith(null);
  });

  it("restores the ordinary install mode and one-shot root from backend resume context", async () => {
    const terminal: OperationSnapshot = {
      ...ACTIVE,
      kind: "update",
      paused: true,
      resume: {
        kind: "install",
        installRoot: "D:\\Selected\\Codex",
        expectation: {
          platform: "windows",
          currentVersion: null,
          targetVersion: "2.0.0",
          packageMoniker: "Codex_2.0.0_x64",
          route: "msix-sideload",
        },
      },
    };
    getSnapshot.mockResolvedValue(null);
    getPausedSnapshot.mockResolvedValue(terminal);
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

    await waitFor(() =>
      expect(setPaused).toHaveBeenCalledWith({
        kind: "install",
        dl: ACTIVE.progress,
        installRoot: "D:\\Selected\\Codex",
        resume: terminal.resume,
      }),
    );
    expect(setBusy).toHaveBeenCalledWith("install");
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
