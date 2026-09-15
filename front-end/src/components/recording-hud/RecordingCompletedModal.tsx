import React from "react";
import {
  CheckCircle2,
  Folder,
  FolderOpen,
  Video,
  Camera,
  Mic,
  Volume2,
  MousePointer,
  Sparkles,
  X,
  ExternalLink,
} from "lucide-react";
import { StopRecordingResult } from "../../lib/types";
import { api } from "../../lib/ipc";
import { useSettingsStore } from "../../stores/settingsStore";

interface RecordingCompletedModalProps {
  result: StopRecordingResult;
  onDismiss: () => void;
}

export const RecordingCompletedModal: React.FC<RecordingCompletedModalProps> = ({
  result,
  onDismiss,
}) => {
  const { selectedSourceId, selectedCameraId, selectedMicId, captureSystemAudio, captureMouse } = useSettingsStore();

  const handleShowInFinder = () => {
    if (result.projectPath) {
      void api.showInFinder(result.projectPath);
    }
  };

  const durationSec = Math.round(result.durationUs / 1_000_000);
  const minutes = Math.floor(durationSec / 60);
  const seconds = durationSec % 60;
  const formattedDuration = `${minutes}m ${seconds.toString().padStart(2, "0")}s`;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 backdrop-blur-md p-4 animate-in fade-in duration-200">
      <div className="w-full max-w-lg rounded-2xl bg-studio-900 border border-studio-750 shadow-2xl p-6 flex flex-col gap-5 text-studio-100">
        {/* Header */}
        <div className="flex items-start justify-between">
          <div className="flex items-center gap-3">
            <div className="w-10 h-10 rounded-xl bg-emerald-500/20 border border-emerald-500/40 flex items-center justify-center text-emerald-400">
              <CheckCircle2 className="w-6 h-6" />
            </div>
            <div>
              <h3 className="text-lg font-bold text-white">Recording Complete!</h3>
              <p className="text-xs text-studio-400">Duration: {formattedDuration}</p>
            </div>
          </div>
          <button
            onClick={onDismiss}
            className="p-1 rounded-lg text-studio-400 hover:text-white hover:bg-studio-800 transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Output Path */}
        <div className="bg-studio-950/80 border border-studio-800 rounded-xl p-3 flex items-center justify-between gap-2 text-xs font-mono">
          <div className="flex items-center gap-2 min-w-0 flex-1">
            <Folder className="w-4 h-4 text-teal-400 shrink-0" />
            <span className="truncate text-studio-300" title={result.projectPath}>
              {result.projectPath}
            </span>
          </div>
          <button
            type="button"
            onClick={handleShowInFinder}
            className="flex items-center gap-1 px-2.5 py-1 rounded bg-studio-800 hover:bg-studio-700 text-studio-200 hover:text-white font-sans text-xs shrink-0 transition-colors"
          >
            <FolderOpen className="w-3.5 h-3.5 text-teal-300" />
            <span>Show in Finder</span>
          </button>
        </div>

        {/* Generated Output Files */}
        <div className="space-y-2">
          <span className="text-xs font-semibold uppercase tracking-wider text-studio-400">
            Recorded Files in Bundle:
          </span>
          <div className="grid grid-cols-1 gap-1.5 text-xs">
            {/* Screen */}
            <div className="flex items-center justify-between px-3 py-2 rounded-lg bg-studio-950/50 border border-studio-800/80">
              <div className="flex items-center gap-2 text-studio-200">
                <Video className="w-4 h-4 text-indigo-400" />
                <span className="font-medium">Screen Recording</span>
              </div>
              <span className="font-mono text-studio-400 text-[11px]">media/screen/000001.mp4</span>
            </div>

            {/* Webcam */}
            {selectedCameraId && (
              <div className="flex items-center justify-between px-3 py-2 rounded-lg bg-studio-950/50 border border-studio-800/80">
                <div className="flex items-center gap-2 text-studio-200">
                  <Camera className="w-4 h-4 text-rose-400" />
                  <span className="font-medium">Webcamera Recording</span>
                </div>
                <span className="font-mono text-studio-400 text-[11px]">media/webcam/000001.mp4</span>
              </div>
            )}

            {/* Microphone */}
            {selectedMicId && (
              <div className="flex items-center justify-between px-3 py-2 rounded-lg bg-studio-950/50 border border-studio-800/80">
                <div className="flex items-center gap-2 text-studio-200">
                  <Mic className="w-4 h-4 text-amber-400" />
                  <span className="font-medium">Microphone Audio</span>
                </div>
                <span className="font-mono text-studio-400 text-[11px]">media/mic/000001.wav</span>
              </div>
            )}

            {/* System Audio */}
            {captureSystemAudio && (
              <div className="flex items-center justify-between px-3 py-2 rounded-lg bg-studio-950/50 border border-studio-800/80">
                <div className="flex items-center gap-2 text-studio-200">
                  <Volume2 className="w-4 h-4 text-emerald-400" />
                  <span className="font-medium">System Audio</span>
                </div>
                <span className="font-mono text-studio-400 text-[11px]">media/system/000001.wav</span>
              </div>
            )}

            {/* Mouse Telemetry (only written for screen recordings with tracking on) */}
            {captureMouse && selectedSourceId && (
              <div className="flex items-center justify-between px-3 py-2 rounded-lg bg-studio-950/50 border border-studio-800/80">
                <div className="flex items-center gap-2 text-studio-200">
                  <MousePointer className="w-4 h-4 text-teal-400" />
                  <span className="font-medium">Mouse Telemetry (Moves & Clicks)</span>
                </div>
                <span className="font-mono text-studio-400 text-[11px]">telemetry/events.jsonl</span>
              </div>
            )}
          </div>
        </div>

        {/* Video Editor Guidance */}
        <div className="rounded-xl bg-teal-950/30 border border-teal-800/50 p-3 text-xs text-teal-300 flex items-start gap-2.5">
          <Sparkles className="w-4 h-4 text-teal-400 shrink-0 mt-0.5" />
          <div className="space-y-1">
            <p className="font-semibold text-teal-200">Ready for AeroShoot Video Editor</p>
            <p className="text-teal-300/80 text-[11px] leading-relaxed">
              Open this recording folder in the AeroShoot Video Editor to automatically generate smart zooms based on your mouse clicks, trim dead air, and export the composite video.
            </p>
          </div>
        </div>

        {/* Actions */}
        <div className="flex items-center justify-end gap-3 pt-2">
          <button
            type="button"
            onClick={onDismiss}
            className="px-4 py-2 rounded-xl bg-studio-800 hover:bg-studio-700 text-studio-200 text-xs font-semibold transition-colors"
          >
            Record Another Session
          </button>
          <button
            type="button"
            onClick={handleShowInFinder}
            className="flex items-center gap-1.5 px-4 py-2 rounded-xl bg-teal-600 hover:bg-teal-500 text-white text-xs font-semibold shadow-md shadow-teal-900/40 transition-colors"
          >
            <ExternalLink className="w-3.5 h-3.5" />
            <span>Open in Finder</span>
          </button>
        </div>
      </div>
    </div>
  );
};
