import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

type Phase = "recording" | "analysing" | "thinking" | "error";

interface OverlayState {
  phase: Phase;
  message?: string;
}

export default function Overlay() {
  const [state, setState] = useState<OverlayState>({ phase: "recording" });
  const [level, setLevel] = useState(0);

  useEffect(() => {
    const stateListener = listen<OverlayState>("overlay-state", (event) => setState(event.payload));
    const waveListener = listen<{ level: number }>("waveform", (event) => {
      setLevel(Math.max(0, Math.min(1, event.payload.level)));
    });
    return () => {
      void stateListener.then((fn) => fn());
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
                height: `${4 + level * weight * 31}px`,
                opacity: 0.42 + level * 0.58,
              }}
            />
          ))}
        </div>
      ) : (
        <div className={state.phase === "error" ? "overlay-error" : "shimmer-label"}>
          {state.message ?? (state.phase === "analysing" ? "Analysing" : "Thinking")}
        </div>
      )}
    </div>
  );
}
