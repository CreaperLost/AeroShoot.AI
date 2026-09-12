import React from "react";
import {
  Video,
  ShieldCheck,
  ShieldAlert,
  Settings,
  Circle,
  Pause,
} from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { PermissionBundle, SessionState } from "../../lib/types";

interface TopNavBarProps {
  permissions?: PermissionBundle;
  onOpenSettings?: () => void;
  sessionState?: SessionState;
}

export const TopNavBar: React.FC<TopNavBarProps> = ({
  permissions,
  onOpenSettings,
  sessionState = "idle",
}) => {
  const { fps, setFps, resolution, setResolution } = useSettingsStore();

  const isPermissionsWarning =
    permissions &&
    (permissions.screenRecording === "denied" ||
      permissions.camera === "denied" ||
      permissions.microphone === "denied");

  const isRecording = sessionState === "recording";
  const isPaused = sessionState === "paused";

  return (
    <header className="studio-top-nav h-14 border-b border-studio-800/80 bg-studio-900/90 backdrop-blur-xl px-5 select-none z-30 shrink-0 flex items-center justify-between">
      {/* 1. Left: Brand & Studio Name */}
      <div className="studio-brand flex min-w-0 items-center space-x-3">
        <div className="flex items-center justify-center w-8 h-8 rounded-xl bg-gradient-to-tr from-rose-600 to-indigo-600 text-white font-black text-sm shadow-md shadow-rose-600/30">
          <Video className="w-4 h-4" />
        </div>
        <div className="flex items-center space-x-2">
          <span className="font-bold text-white tracking-tight text-sm">AeroShoot</span>
          <span className="text-[10px] px-2 py-0.5 rounded-full bg-rose-500/15 border border-rose-500/30 text-rose-300 font-mono font-medium">
            Recorder
          </span>
        </div>
      </div>

      {/* 2. Center: Status Badge */}
      <div className="flex items-center bg-studio-950/80 px-3 py-1.5 rounded-xl border border-studio-800 shadow-inner">
        {isRecording ? (
          <div className="flex items-center space-x-2 text-xs font-semibold text-rose-300">
            <span className="relative flex h-2.5 w-2.5">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-rose-400 opacity-75"></span>
              <span className="relative inline-flex rounded-full h-2.5 w-2.5 bg-rose-500"></span>
            </span>
            <span>Recording Active</span>
          </div>
        ) : isPaused ? (
          <div className="flex items-center space-x-2 text-xs font-semibold text-amber-300">
            <Pause className="w-3 h-3 text-amber-400" />
            <span>Recording Paused</span>
          </div>
        ) : (
          <div className="flex items-center space-x-2 text-xs font-medium text-studio-400">
            <Circle className="w-2 h-2 text-emerald-500 fill-emerald-500" />
            <span>Ready to Record</span>
          </div>
        )}
      </div>

      {/* 3. Right: Quality Controls & Status */}
      <div className="studio-quality flex items-center justify-self-end space-x-3">
        {/* FPS & Quality Toggle */}
        <div className="flex items-center bg-studio-950/60 border border-studio-800 rounded-lg p-0.5 text-[11px] font-mono">
          <button
            onClick={() => setFps(30)}
            disabled={isRecording || isPaused}
            className={`px-2 py-0.5 rounded transition-colors disabled:opacity-50 ${
              fps === 30 ? "bg-studio-800 text-white font-medium shadow-sm" : "text-studio-400 hover:text-studio-200"
            }`}
          >
            30 FPS
          </button>
          <button
            onClick={() => setFps(60)}
            disabled={isRecording || isPaused}
            className={`px-2 py-0.5 rounded transition-colors disabled:opacity-50 ${
              fps === 60 ? "bg-studio-800 text-white font-medium shadow-sm" : "text-studio-400 hover:text-studio-200"
            }`}
          >
            60 FPS
          </button>
          <div className="h-3 w-px bg-studio-800 mx-0.5" />
          <button
            onClick={() => setResolution(resolution === "1080p" ? "4K" : "1080p")}
            disabled={isRecording || isPaused}
            className="px-2 py-0.5 rounded text-indigo-400 hover:text-indigo-300 font-medium disabled:opacity-50"
          >
            {resolution}
          </button>
        </div>

        {/* Permissions indicator */}
        {isPermissionsWarning ? (
          <button
            onClick={onOpenSettings}
            className="flex items-center space-x-1.5 px-2.5 py-1 rounded-lg bg-rose-950/50 border border-rose-800/60 text-rose-300 text-xs font-medium hover:bg-rose-900/50 transition-colors"
            title="System permissions require attention"
          >
            <ShieldAlert className="w-3.5 h-3.5 text-rose-400" />
            <span>Permissions</span>
          </button>
        ) : (
          <div
            className="flex items-center space-x-1 px-2 py-1 rounded-lg bg-studio-950/40 border border-studio-800/80 text-emerald-400 text-xs"
            title="All capture pipelines ready"
          >
            <ShieldCheck className="w-3.5 h-3.5 text-emerald-400" />
            <span className="text-[11px] text-studio-400 font-mono">Ready</span>
          </div>
        )}

        {/* Quick Settings */}
        {onOpenSettings && (
          <button
            onClick={onOpenSettings}
            className="p-1.5 rounded-lg bg-studio-850 hover:bg-studio-800 border border-studio-800 text-studio-400 hover:text-white transition-colors"
            title="Preferences"
          >
            <Settings className="w-4 h-4" />
          </button>
        )}
      </div>
    </header>
  );
};
