import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as Progress from "@radix-ui/react-progress";
import type { WorkflowStateSnapshot } from "./types";

type Phase = "recording" | "analysing" | "thinking" | "error";

interface OverlayState {
  phase: Phase;
  message?: string;
}

export default function Overlay() {
  const [state, setState] = useState<OverlayState>({ phase: "recording" });
  const [level, setLevel] = useState(0);
  const [progress, setProgress] = useState(0);
  const [appearanceKey, setAppearanceKey] = useState(0);
  const [isClosing, setIsClosing] = useState(false);
  const targetLevel = useRef(0);
  const displayedLevel = useRef(0);
  const lastPublishedLevel = useRef(0);
  const lastPublishedProgress = useRef(0);
  const processingStartedAt = useRef<number | null>(null);
  const processingComplete = useRef(false);
  const currentSessionId = useRef<number | null>(null);
  const lastRevision = useRef(0);

  useEffect(() => {
    let animationFrame = 0;
    const publishProgress = (nextProgress: number) => {
      if (nextProgress !== lastPublishedProgress.current) {
        lastPublishedProgress.current = nextProgress;
        setProgress(nextProgress);
      }
    };
    const animate = () => {
      animationFrame = 0;
      const difference = targetLevel.current - displayedLevel.current;
      const smoothing = difference > 0 ? 0.34 : 0.16;
      displayedLevel.current += difference * smoothing;
      if (Math.abs(difference) < 0.002) {
        displayedLevel.current = targetLevel.current;
      }
      if (displayedLevel.current !== lastPublishedLevel.current) {
        lastPublishedLevel.current = displayedLevel.current;
        setLevel(displayedLevel.current);
      }

      const isProcessing =
        processingStartedAt.current !== null && !processingComplete.current;
      if (isProcessing) {
        const elapsed = performance.now() - processingStartedAt.current!;
        const fastPhaseDuration = 650;

        if (elapsed <= fastPhaseDuration) {
          const fastPhase = elapsed / fastPhaseDuration;
          const easedFastPhase = 1 - Math.pow(1 - fastPhase, 3);
          publishProgress(70 * easedFastPhase);
        } else {
          const slowPhaseElapsed = elapsed - fastPhaseDuration;
          publishProgress(70 + 25 * (1 - Math.exp(-slowPhaseElapsed / 2200)));
        }
      }

      if (displayedLevel.current !== targetLevel.current || isProcessing) {
        animationFrame = window.requestAnimationFrame(animate);
      }
    };
    const scheduleAnimation = () => {
      if (animationFrame === 0) {
        animationFrame = window.requestAnimationFrame(animate);
      }
    };

    const stateListener = listen<OverlayState>("overlay-state", (event) => {
      const nextState = event.payload;
      setState(nextState);

      if (nextState.phase === "analysing") {
        processingStartedAt.current = performance.now();
        processingComplete.current = false;
        publishProgress(0);
        scheduleAnimation();
      } else if (nextState.phase === "recording" || nextState.phase === "error") {
        setIsClosing(false);
        setAppearanceKey((current) => current + 1);
        processingStartedAt.current = null;
        processingComplete.current = false;
        publishProgress(0);
        scheduleAnimation();
      }
    });

    const workflowListener = listen<WorkflowStateSnapshot>("workflow-state", (event) => {
      const payload = event.payload;
      // Drop stale snapshots before any session reset or phase processing:
      // an out-of-order event from a previous revision must never modify the
      // current session's closing state.
      if (payload.revision < lastRevision.current) {
        return;
      }
      lastRevision.current = payload.revision;
      const { session_id, phase } = payload;
      if (session_id !== null && session_id !== currentSessionId.current) {
        currentSessionId.current = session_id;
        setAppearanceKey((k) => k + 1);
        processingStartedAt.current = null;
        processingComplete.current = false;
        targetLevel.current = 0;
        displayedLevel.current = 0;
        publishProgress(0);
        // A new session must never inherit the previous session's closing state.
        setIsClosing(false);
      }
      if (phase === "idle") {
        setIsClosing(true);
      }
    });

    const dismissalListener = listen("overlay-dismiss", () => {
      setIsClosing(true);
    });

    const completionListener = listen("overlay-progress-complete", () => {
      processingComplete.current = true;
      publishProgress(100);
      scheduleAnimation();
    });

    const waveListener = listen<{ level: number }>("waveform", (event) => {
      const rawLevel = Math.max(0, Math.min(1, event.payload.level));
      targetLevel.current = rawLevel < 0.012
        ? 0
        : Math.min(1, Math.pow(rawLevel, 0.7) * 1.28);
      scheduleAnimation();
    });

    return () => {
      window.cancelAnimationFrame(animationFrame);
      void stateListener.then((fn) => fn());
      void workflowListener.then((fn) => fn());
      void dismissalListener.then((fn) => fn());
      void completionListener.then((fn) => fn());
      void waveListener.then((fn) => fn());
    };
  }, []);

  const waveformProfile = [0.12, 0.24, 0.43, 0.72, 0.94, 1, 0.84, 0.61, 0.4, 0.24, 0.13];
  const processingLabel =
    state.message ?? (state.phase === "analysing" ? "Analyzing" : "Thinking");

  return (
    <div
      key={appearanceKey}
      className={`overlay-bar overlay-bar--${state.phase}${isClosing ? " overlay-bar--closing" : ""}`}
    >
      {state.phase === "recording" ? (
        <div className="waveform" aria-label="Recording">
          {waveformProfile.map((weight, index) => (
            <i
              key={index}
              style={{
                height: `${3 + level * weight * 19}px`,
                opacity: 0.5 + level * 0.5,
              }}
            />
          ))}
        </div>
      ) : state.phase === "error" ? (
        <div className="overlay-error">{state.message}</div>
      ) : (
        <div className="processing-indicator">
          <span className="shimmer-label" data-label={processingLabel}>
            {processingLabel}
          </span>
          <Progress.Root
            className="processing-progress"
            value={progress}
            aria-label="Preparing transcription"
          >
            <Progress.Indicator
              className="processing-progress__fill"
              style={{ transform: `translateX(-${100 - progress}%)` }}
            />
          </Progress.Root>
        </div>
      )}
    </div>
  );
}
