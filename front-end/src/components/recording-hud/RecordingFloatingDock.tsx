import React, { useEffect, useState } from "react";
import { Circle, Square, Pause, Play, AlertTriangle, Loader2 } from "lucide-react";
import { SessionState } from "../../lib/types";

interface RecordingFloatingDockProps {
  sessionState: SessionState;
  elapsedMs: number;
  /** When the start countdown reaches zero (ms since epoch); null without one. */
  countdownEndsAt?: number | null;
  canStart: boolean;
  sessionOwned?: boolean;
  disabledReason?: string;
  onStart: () => void;
  onPause: () => void;
  onResume: () => void;
  onStop: () => void;
}

/** Whole seconds left until `endsAt`, refreshed while a countdown runs. */
function useSecondsUntil(endsAt: number | null): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (endsAt === null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(timer);
  }, [endsAt]);
  return endsAt === null ? null : Math.max(0, Math.ceil((endsAt - now) / 1000));
}

export const RecordingFloatingDock: React.FC<RecordingFloatingDockProps> = ({
  sessionState,
  elapsedMs,
  countdownEndsAt = null,
  canStart,
  sessionOwned = false,
  disabledReason,
  onStart,
  onPause,
  onResume,
  onStop,
}) => {
  const isRecording = sessionState === "recording";
  const isPaused = sessionState === "paused";
  const isStarting = sessionState === "preparing";
  const isStopping = sessionState === "stopping";
  const isTransitioning = isStarting || isStopping;
  const showRecoveryStop = sessionOwned && sessionState === "error";
  const countdown = useSecondsUntil(isStarting ? countdownEndsAt : null);
  const counting = countdown !== null && countdown > 0;

  const formatElapsed = (ms: number) => {
    const totalSecs = Math.floor(ms / 1000);
    const m = Math.floor(totalSecs / 60)
      .toString()
      .padStart(2, "0");
    const s = (totalSecs % 60).toString().padStart(2, "0");
    const centis = Math.floor((ms % 1000) / 10)
      .toString()
      .padStart(2, "0");
    return `${m}:${s}.${centis}`;
  };

  return (
    <div className="flex w-full flex-col gap-2 select-none">
      <div className="flex w-full items-center gap-2">
        {/* Timer & Status Badge */}
        <div className="flex shrink-0 items-center gap-2.5 px-3 py-2 rounded-xl bg-studio-950/80 border border-studio-800 font-mono text-sm">
          {isRecording && (
            <span className="relative flex h-2.5 w-2.5">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-rose-400 opacity-75" />
              <span className="relative inline-flex rounded-full h-2.5 w-2.5 bg-rose-500" />
            </span>
          )}
          {isPaused && <span className="h-2.5 w-2.5 rounded-full bg-amber-400" />}
          {!isRecording && !isPaused && <span className="h-2.5 w-2.5 rounded-full bg-studio-500" />}

          <span
            className={`font-semibold ${
              isRecording ? "text-rose-400" : isPaused ? "text-amber-400" : "text-studio-300"
            }`}
          >
            {formatElapsed(elapsedMs)}
          </span>
        </div>

        {/* Main Trigger Button */}
        {isStopping ? (
          <button
            type="button"
            disabled
            className="flex flex-1 items-center justify-center gap-2 px-4 py-2.5 rounded-xl bg-studio-800 border border-studio-700 text-studio-200 text-sm font-semibold"
            title="Finishing the recording files"
          >
            <Loader2 className="w-4 h-4 animate-spin" />
            <span>Saving recording…</span>
          </button>
        ) : showRecoveryStop ? (
          <button
            type="button"
            onClick={onStop}
            className="flex flex-1 items-center justify-center gap-2 px-4 py-2.5 rounded-xl bg-gradient-to-r from-rose-700/80 to-rose-600 hover:from-rose-600 hover:to-rose-500 border border-rose-500/50 text-white text-sm font-semibold transition-colors shadow-lg shadow-rose-950/50"
            title="Retry stop and keep the recoverable project"
          >
            <Square className="w-3.5 h-3.5 fill-white" />
            <span>Retry Stop</span>
          </button>
        ) : !isRecording && !isPaused ? (
          <button
            type="button"
            onClick={onStart}
            disabled={!canStart || isTransitioning}
            title={isStarting ? "Starting capture" : disabledReason || "Start Recording Session"}
            aria-live={isStarting ? "polite" : undefined}
            className="flex flex-1 items-center justify-center gap-2 px-5 py-2.5 rounded-xl bg-gradient-to-r from-rose-600 to-rose-500 hover:from-rose-500 hover:to-rose-400 disabled:opacity-50 disabled:cursor-not-allowed text-white text-sm font-semibold shadow-xl shadow-rose-900/40 transition-colors active:scale-[0.98]"
          >
            {isStarting && !counting ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Circle className="w-4 h-4 fill-white" />
            )}
            <span>{isStarting ? (counting ? `Recording in ${countdown}…` : "Starting…") : "Record"}</span>
          </button>
        ) : (
          <>
            {/* Pause / Resume */}
            {isRecording ? (
              <button
                type="button"
                onClick={onPause}
                className="shrink-0 p-2.5 rounded-xl bg-studio-800 hover:bg-studio-700 border border-studio-700 text-amber-400 transition-colors shadow"
                title="Pause Recording"
                aria-label="Pause recording"
              >
                <Pause className="w-4 h-4" />
              </button>
            ) : (
              <button
                type="button"
                onClick={onResume}
                className="shrink-0 p-2.5 rounded-xl bg-studio-800 hover:bg-studio-700 border border-studio-700 text-emerald-400 transition-colors shadow"
                title="Resume Recording"
                aria-label="Resume recording"
              >
                <Play className="w-4 h-4 fill-current" />
              </button>
            )}

            {/* Stop and Open Edit Studio */}
            <button
              type="button"
              onClick={onStop}
              className="flex flex-1 items-center justify-center gap-2 px-4 py-2.5 rounded-xl bg-gradient-to-r from-rose-700/80 to-rose-600 hover:from-rose-600 hover:to-rose-500 border border-rose-500/50 text-white text-sm font-semibold transition-colors shadow-lg shadow-rose-950/50 active:scale-[0.98]"
              title="Stop capture and edit in Studio"
            >
              <Square className="w-3.5 h-3.5 fill-white" />
              <span>Stop &amp; Edit</span>
            </button>
          </>
        )}
      </div>

      {!showRecoveryStop && !isRecording && !isPaused && !isTransitioning && !canStart && disabledReason && (
        <div
          className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-amber-950/40 border border-amber-800/40 text-[11px] text-amber-300"
          title={disabledReason}
        >
          <AlertTriangle className="w-3.5 h-3.5 text-amber-400 shrink-0" />
          <span className="min-w-0">{disabledReason}</span>
        </div>
      )}
    </div>
  );
};
