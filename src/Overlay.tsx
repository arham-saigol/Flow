import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as Progress from "@radix-ui/react-progress";

type Phase = "recording" | "analysing" | "thinking" | "error";

interface OverlayState {
  phase: Phase;
  message?: string;
}

export default function Overlay() {
  const [state, setState] = useState<OverlayState>({ phase: "recording" });
  const [level, setLevel] = useState(0);
  const [progress, setProgress] = useState(0);
  const targetLevel = useRef(0);
  const displayedLevel = useRef(0);
  const processingStartedAt = useRef<number | null>(null);
  const processingComplete = useRef(false);

  useEffect(() => {
    const stateListener = listen<OverlayState>("overlay-state", (event) => {
      const nextState = event.payload;
      setState(nextState);

      if (nextState.phase === "analysing") {
        processingStartedAt.current = performance.now();
        processingComplete.current = false;
        setProgress(0);
      } else if (nextState.phase === "recording" || nextState.phase === "error") {
        processingStartedAt.current = null;
        processingComplete.current = false;
        setProgress(0);
      }
    });
    const completionListener = listen("overlay-progress-complete", () => {
      processingComplete.current = true;
      setProgress(100);
    });
    const waveListener = listen<{ level: number }>("waveform", (event) => {
      const rawLevel = Math.max(0, Math.min(1, event.payload.level));
      targetLevel.current = rawLevel < 0.012
        ? 0
        : Math.min(1, Math.pow(rawLevel, 0.7) * 1.28);
    });
    let animationFrame = 0;
    const animate = () => {
      const difference = targetLevel.current - displayedLevel.current;
      const smoothing = difference > 0 ? 0.34 : 0.16;
      displayedLevel.current += difference * smoothing;
      if (Math.abs(difference) < 0.002) {
        displayedLevel.current = targetLevel.current;
      }
      setLevel(displayedLevel.current);

      if (processingStartedAt.current !== null && !processingComplete.current) {
        const elapsed = performance.now() - processingStartedAt.current;
        const fastPhaseDuration = 650;

        if (elapsed <= fastPhaseDuration) {
          const fastPhase = elapsed / fastPhaseDuration;
          const easedFastPhase = 1 - Math.pow(1 - fastPhase, 3);
          setProgress(70 * easedFastPhase);
        } else {
          const slowPhaseElapsed = elapsed - fastPhaseDuration;
          setProgress(70 + 25 * (1 - Math.exp(-slowPhaseElapsed / 2200)));
        }
      }

      animationFrame = window.requestAnimationFrame(animate);
    };
    animationFrame = window.requestAnimationFrame(animate);

    return () => {
      window.cancelAnimationFrame(animationFrame);
      void stateListener.then((fn) => fn());
      void completionListener.then((fn) => fn());
      void waveListener.then((fn) => fn());
    };
  }, []);

  const waveformProfile = [0.12, 0.24, 0.43, 0.72, 0.94, 1, 0.84, 0.61, 0.4, 0.24, 0.13];

  return (
    <div className={`overlay-bar overlay-bar--${state.phase}`}>
      {state.phase === "recording" ? (
        <div className="waveform" aria-label="Recording. Press Escape to cancel.">
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
          <span className="shimmer-label">
            {state.message ?? (state.phase === "analysing" ? "Analyzing" : "Thinking")}
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
