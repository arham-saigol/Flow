import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

type Phase = "starting" | "recording" | "analysing" | "thinking" | "error";

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
  const [notice, setNotice] = useState<string | null>(null);
  const noticeTimeout = useRef<number | null>(null);
  const targetLevel = useRef(0);
  const displayedLevel = useRef(0);
  const lastPublishedLevel = useRef(0);
  const lastPublishedProgress = useRef(0);
  const processingStartedAt = useRef<number | null>(null);
  const processingComplete = useRef(false);

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
      setNotice(null);
      setState(nextState);

      if (nextState.phase === "analysing") {
        processingStartedAt.current = performance.now();
        processingComplete.current = false;
        publishProgress(0);
        scheduleAnimation();
      } else if (
        nextState.phase === "starting" ||
        nextState.phase === "recording" ||
        nextState.phase === "error"
      ) {
        setIsClosing(false);
        setAppearanceKey((current) => current + 1);
        processingStartedAt.current = null;
        processingComplete.current = false;
        publishProgress(0);
        scheduleAnimation();
      }
    });
    const noticeListener = listen<{ message: string }>("overlay-notice", (event) => {
      setNotice(event.payload.message);
      if (noticeTimeout.current !== null) {
        window.clearTimeout(noticeTimeout.current);
      }
      noticeTimeout.current = window.setTimeout(() => {
        setNotice(null);
        noticeTimeout.current = null;
      }, 900);
    });
    const dismissalListener = listen("overlay-dismiss", () => {
      setNotice(null);
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
      if (noticeTimeout.current !== null) {
        window.clearTimeout(noticeTimeout.current);
      }
      void stateListener.then((fn) => fn());
      void noticeListener.then((fn) => fn());
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
      {notice !== null ? (
        <div className="overlay-notice" role="status">{notice}</div>
      ) : state.phase === "recording" ? (
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
          <span className="shimmer-label" data-label={processingLabel}>
            {processingLabel}
          </span>
          <div
            className="processing-progress"
            role="progressbar"
            aria-label="Preparing transcription"
            aria-valuenow={progress}
            aria-valuemin={0}
            aria-valuemax={100}
          >
            <div
              className="processing-progress__fill"
              style={{ transform: `translateX(-${100 - progress}%)` }}
            />
          </div>
        </div>
      )}
    </div>
  );
}
