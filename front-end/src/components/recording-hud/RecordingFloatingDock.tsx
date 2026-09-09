import React from "react";
import { Circle, Square, Pause, Play, AlertTriangle } from "lucide-react";
import { SessionState } from "../../lib/types";

interface RecordingFloatingDockProps {
  sessionState: SessionState;
  elapsedMs: number;
  canStart: boolean;
  disabledReason?: string;
  onStart: () => void;
  onPause: () => void;
  onResume: () => void;
  onStop: () => void;
}

export const RecordingFloatingDock: React.FC<RecordingFloatingDockProps> = ({
  sessionState,
  elapsedMs,
  canStart,
  disabledReason,
  onStart,
  onPause,
  onResume,
  onStop,
}) => {
  const isRecording = sessionState === "recording";
  const isPaused = sessionState === "paused";
  const isTransitioning = sessionState === "preparing" || sessionState === "stopping";

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
    <div className="flex items-center space-x-3 bg-studio-900/95 border border-studio-750/90 backdrop-blur-xl px-5 py-2.5 rounded-2xl shadow-2xl shadow-black/80 z-30 select-none">
      {/* Timer & Status Badge */}
      <div className="flex items-center space-x-2.5 px-3 py-1.5 rounded-xl bg-studio-950/80 border border-studio-800 font-mono text-sm">
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
      {!isRecording && !isPaused ? (
        <div className="flex items-center space-x-2">
          <button
            type="button"
            onClick={onStart}
            disabled={!canStart || isTransitioning}
            title={disabledReason || "Start Recording Session"}
            className="flex items-center space-x-2 px-5 py-2.5 rounded-xl bg-gradient-to-r from-rose-600 to-rose-500 hover:from-rose-500 hover:to-rose-400 disabled:opacity-50 disabled:cursor-not-allowed text-white text-sm font-semibold shadow-xl shadow-rose-900/40 transition-all hover:scale-105 active:scale-95"
          >
            <Circle className="w-4 h-4 fill-white" />
            <span>{isTransitioning ? "Preparing..." : "Record"}</span>
          </button>

          {!canStart && disabledReason && (
            <div
              className="flex items-center space-x-1 px-2.5 py-1.5 rounded-lg bg-amber-950/40 border border-amber-800/40 text-[11px] text-amber-300 max-w-xs truncate"
              title={disabledReason}
            >
              <AlertTriangle className="w-3.5 h-3.5 text-amber-400 shrink-0" />
              <span className="truncate">{disabledReason}</span>
            </div>
          )}
        </div>
      ) : (
        <div className="flex items-center space-x-2">
          {/* Pause / Resume */}
          {isRecording ? (
            <button
              type="button"
              onClick={onPause}
              className="p-2.5 rounded-xl bg-studio-800 hover:bg-studio-700 border border-studio-700 text-amber-400 transition-colors shadow"
              title="Pause Recording"
            >
              <Pause className="w-4 h-4" />
            </button>
          ) : (
            <button
              type="button"
              onClick={onResume}
              className="p-2.5 rounded-xl bg-studio-800 hover:bg-studio-700 border border-studio-700 text-emerald-400 transition-colors shadow"
              title="Resume Recording"
            >
              <Play className="w-4 h-4 fill-current" />
            </button>
          )}

          {/* Stop and Open Edit Studio */}
          <button
            type="button"
            onClick={onStop}
            className="flex items-center space-x-2 px-4 py-2.5 rounded-xl bg-gradient-to-r from-rose-700/80 to-rose-600 hover:from-rose-600 hover:to-rose-500 border border-rose-500/50 text-white text-sm font-semibold transition-all shadow-lg shadow-rose-950/50 hover:scale-102 active:scale-98"
            title="Stop capture and edit in Studio"
          >
            <Square className="w-3.5 h-3.5 fill-white" />
            <span>Stop &amp; Edit</span>
          </button>
        </div>
      )}
    </div>
  );
};
