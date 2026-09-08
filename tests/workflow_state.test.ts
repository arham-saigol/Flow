import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import type { WorkflowStateSnapshot } from "../src/types";

let mockIsTauri = false;
let mockEventCallback: ((event: { payload: WorkflowStateSnapshot }) => void) | undefined;
let mockWorkflowStateResult: WorkflowStateSnapshot | null = null;
// Optional deferred implementations used by subscribe-first ordering tests.
let mockListenImpl: ((cb: (event: { payload: WorkflowStateSnapshot }) => void) => Promise<() => void>) | null = null;
let mockWorkflowStateImpl: (() => Promise<WorkflowStateSnapshot>) | null = null;

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => mockIsTauri,
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((_event: string, cb: (event: { payload: WorkflowStateSnapshot }) => void) => {
    mockEventCallback = cb;
    if (mockListenImpl) {
      return mockListenImpl(cb);
    }
    return Promise.resolve(() => {});
  }),
}));

vi.mock("../src/api", () => ({
  api: {
    workflowState: () => {
      if (mockWorkflowStateImpl) {
        return mockWorkflowStateImpl();
      }
      return mockWorkflowStateResult
        ? Promise.resolve(mockWorkflowStateResult)
        : Promise.reject(new Error("No snapshot"));
    },
  },
}));

// Import after mocks
import { useWorkflowState } from "../src/hooks/useWorkflowState";

describe("useWorkflowState Hook (F22)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockIsTauri = false;
    mockEventCallback = undefined;
    mockWorkflowStateResult = null;
    mockListenImpl = null;
    mockWorkflowStateImpl = null;
  });

  it("initializes with idle phase when not running in Tauri", () => {
    mockIsTauri = false;

    const { result } = renderHook(() => useWorkflowState());

    expect(result.current.phase).toBe("idle");
    expect(result.current.isIdle).toBe(true);
    expect(result.current.isRecording).toBe(false);
    expect(result.current.isProcessing).toBe(false);
  });

  it("fetches initial workflow snapshot on mount and orders by revision", async () => {
    mockIsTauri = true;

    const initialSnapshot: WorkflowStateSnapshot = {
      revision: 10,
      session_id: 1,
      phase: "recording",
      active_pending_id: null,
      can_start: false,
      can_stop: true,
      can_cancel: true,
      message_code: null,
    };
    mockWorkflowStateResult = initialSnapshot;

    const { result } = renderHook(() => useWorkflowState());

    // Wait for promise resolution
    await act(async () => {
      await Promise.resolve();
    });

    expect(result.current.revision).toBe(10);
    expect(result.current.phase).toBe("recording");
    expect(result.current.isRecording).toBe(true);

    // Out-of-order event with lower revision (e.g. 8) should be ignored
    act(() => {
      mockEventCallback?.({
        payload: {
          ...initialSnapshot,
          revision: 8,
          phase: "idle",
        },
      });
    });

    expect(result.current.revision).toBe(10);
    expect(result.current.phase).toBe("recording");

    // Higher revision event (e.g. 11) should be accepted
    act(() => {
      mockEventCallback?.({
        payload: {
          ...initialSnapshot,
          revision: 11,
          phase: "transcribing",
        },
      });
    });

    expect(result.current.revision).toBe(11);
    expect(result.current.phase).toBe("transcribing");
    expect(result.current.isProcessing).toBe(true);
  });

  it("subscribes before fetching the snapshot and keeps the greatest revision", async () => {
    mockIsTauri = true;

    const initialSnapshot: WorkflowStateSnapshot = {
      revision: 10,
      session_id: 1,
      phase: "recording",
      active_pending_id: null,
      can_start: false,
      can_stop: true,
      can_cancel: true,
      message_code: null,
    };
    const laterSnapshot: WorkflowStateSnapshot = {
      ...initialSnapshot,
      revision: 11,
      phase: "transcribing",
    };

    // Deferred promises for both listen and workflowState.
    let resolveListen!: (unlisten: () => void) => void;
    const listenDeferred = new Promise<() => void>((resolve) => {
      resolveListen = resolve;
    });
    let resolveSnapshot!: (snapshot: WorkflowStateSnapshot) => void;
    const snapshotDeferred = new Promise<WorkflowStateSnapshot>((resolve) => {
      resolveSnapshot = resolve;
    });

    let listenSettled = false;
    mockListenImpl = () =>
      listenDeferred.then((unlisten) => {
        listenSettled = true;
        return unlisten;
      });
    mockWorkflowStateImpl = () => snapshotDeferred;

    const { result } = renderHook(() => useWorkflowState());

    // The handler is registered synchronously, but the snapshot request must
    // begin only after the subscription promise resolves.
    expect(mockEventCallback).toBeDefined();
    expect(listenSettled).toBe(false);
    expect(result.current.revision).toBe(0);

    resolveListen(() => {});
    await act(async () => {
      await listenDeferred;
      await Promise.resolve();
    });
    expect(listenSettled).toBe(true);

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(result.current.revision).toBe(0);

    // Revision 11 arrives BEFORE the revision 10 snapshot resolves.
    act(() => {
      mockEventCallback?.({ payload: laterSnapshot });
    });
    expect(result.current.revision).toBe(11);
    expect(result.current.phase).toBe("transcribing");

    resolveSnapshot(initialSnapshot);
    await act(async () => {
      await snapshotDeferred;
      await Promise.resolve();
    });

    // Greatest-revision preservation: the older snapshot must not win.
    expect(result.current.revision).toBe(11);
    expect(result.current.phase).toBe("transcribing");
    expect(result.current.isProcessing).toBe(true);
  });
});
