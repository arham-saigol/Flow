import { useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isTauri } from "@tauri-apps/api/core";
import { api } from "../api";
import type { WorkflowStateSnapshot } from "../types";

const INITIAL_SNAPSHOT: WorkflowStateSnapshot = {
  revision: 0,
  session_id: null,
  phase: "idle",
  active_pending_id: null,
  can_start: true,
  can_stop: false,
  can_cancel: false,
  message_code: null,
};

export function useWorkflowState() {
  const [snapshot, setSnapshot] = useState<WorkflowStateSnapshot>(INITIAL_SNAPSHOT);
  const revisionRef = useRef<number>(0);

  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: UnlistenFn | undefined;
    let isMounted = true;

    // Await listener installation BEFORE fetching initial snapshot to eliminate subscription race
    listen<WorkflowStateSnapshot>("workflow-state", (event) => {
      if (!isMounted) return;
      const next = event.payload;
      if (next.revision > revisionRef.current) {
        revisionRef.current = next.revision;
        setSnapshot(next);
      }
    })
      .then((unlistenFn) => {
        if (!isMounted) {
          unlistenFn();
          return;
        }
        unlisten = unlistenFn;
        return api.workflowState();
      })
      .then((initial) => {
        if (!isMounted || !initial) return;
        if (initial.revision >= revisionRef.current) {
          revisionRef.current = initial.revision;
          setSnapshot(initial);
        }
      })
      .catch((err) => {
        console.error("Failed to subscribe or fetch initial workflow state:", err);
      });

    return () => {
      isMounted = false;
      if (unlisten) {
        unlisten();
      }
    };
  }, []);

  return {
    ...snapshot,
    isIdle: snapshot.phase === "idle",
    isRecording: snapshot.phase === "recording",
    isProcessing:
      snapshot.phase === "transcribing" ||
      snapshot.phase === "cleaning" ||
      snapshot.phase === "delivering",
    isFaulted: snapshot.phase === "faulted",
  };
}
