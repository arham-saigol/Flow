import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import type { WorkflowStateSnapshot } from "../src/types";

let mockIsTauri = false;
let mockEventCallback: ((event: { payload: WorkflowStateSnapshot }) => void) | undefined;
let mockWorkflowStateResult: WorkflowStateSnapshot | null = null;

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => mockIsTauri,
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((_event: string, cb: (event: { payload: WorkflowStateSnapshot }) => void) => {
    mockEventCallback = cb;
    return Promise.resolve(() => {});
  }),
}));

vi.mock("../src/api", () => ({
  api: {
    workflowState: () =>
      mockWorkflowStateResult
        ? Promise.resolve(mockWorkflowStateResult)
        : Promise.reject(new Error("No snapshot")),
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
});
