import React from "react";
import {
  Video,
  Film,
  ShieldCheck,
  ShieldAlert,
  Settings,
} from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { PermissionBundle } from "../../lib/types";

interface TopNavBarProps {
  permissions?: PermissionBundle;
  onOpenSettings?: () => void;
  sessionLocked?: boolean;
}

export const TopNavBar: React.FC<TopNavBarProps> = ({
  permissions,
  onOpenSettings,
  sessionLocked = false,
}) => {
  const { activeScene, setActiveScene, fps, setFps, resolution, setResolution } = useSettingsStore();

  const isPermissionsWarning =
    permissions &&
    (permissions.screenRecording === "denied" ||
      permissions.camera === "denied" ||
      permissions.microphone === "denied");

  return (
    <header className="studio-top-nav h-14 border-b border-studio-800/80 bg-studio-900/90 backdrop-blur-xl px-5 select-none z-30 shrink-0">
      {/* 1. Left: Brand & Studio Name */}
      <div className="studio-brand flex min-w-0 items-center space-x-3">
        <div className="flex items-center justify-center w-8 h-8 rounded-xl bg-gradient-to-tr from-indigo-600 to-indigo-400 text-white font-black text-sm shadow-md shadow-indigo-600/30">
          ▲
        </div>
        <div className="flex items-center space-x-2">
          <span className="font-bold text-white tracking-tight text-sm">AeroShoot</span>
          <span className="text-[10px] px-2 py-0.5 rounded-full bg-indigo-500/15 border border-indigo-500/30 text-indigo-300 font-mono font-medium">
            AI Studio
          </span>
        </div>
      </div>

      {/* 2. Center: Scene Switcher Card (Top Card for Record / Edit Scene Switch) */}
      <div className="flex items-center bg-studio-950/80 p-1 rounded-xl border border-studio-800 shadow-inner">
        <button
          onClick={() => setActiveScene("record")}
          className={`flex items-center space-x-2 px-4 py-1.5 rounded-lg text-xs font-semibold transition-all duration-200 ${
            activeScene === "record"
              ? "bg-gradient-to-r from-rose-600/90 to-rose-500 text-white shadow-lg shadow-rose-900/30 scale-[1.02]"
              : "text-studio-400 hover:text-studio-200 hover:bg-studio-850/50"
          }`}
        >
          <span
            className={`w-2 h-2 rounded-full ${
              activeScene === "record" ? "bg-white animate-pulse" : "bg-rose-500/70"
            }`}
          />
          <Video className="w-3.5 h-3.5" />
          <span>Record Scene</span>
        </button>

        <button
          type="button"
          onClick={() => {
            if (!sessionLocked) setActiveScene("edit");
          }}
          disabled={sessionLocked}
          title={
            sessionLocked
              ? "Stop or recover the recording session before opening Edit Studio"
              : "Edit Studio"
          }
          className={`flex items-center space-x-2 px-4 py-1.5 rounded-lg text-xs font-semibold transition-all duration-200 ${
            activeScene === "edit"
              ? "bg-gradient-to-r from-indigo-600 to-indigo-500 text-white shadow-lg shadow-indigo-900/30 scale-[1.02]"
              : sessionLocked
                ? "text-studio-600 cursor-not-allowed"
                : "text-studio-400 hover:text-studio-200 hover:bg-studio-850/50"
          }`}
        >
          <Film className="w-3.5 h-3.5" />
          <span>Edit Studio</span>
        </button>
      </div>

      {/* 3. Right: Quality Controls & Status */}
      <div className="studio-quality flex items-center justify-self-end space-x-3">
        {/* FPS & Quality Toggle */}
        <div className="flex items-center bg-studio-950/60 border border-studio-800 rounded-lg p-0.5 text-[11px] font-mono">
          <button
            onClick={() => setFps(30)}
            className={`px-2 py-0.5 rounded transition-colors ${
              fps === 30 ? "bg-studio-800 text-white font-medium shadow-sm" : "text-studio-400 hover:text-studio-200"
            }`}
          >
            30 FPS
          </button>
          <button
            onClick={() => setFps(60)}
            className={`px-2 py-0.5 rounded transition-colors ${
              fps === 60 ? "bg-studio-800 text-white font-medium shadow-sm" : "text-studio-400 hover:text-studio-200"
            }`}
          >
            60 FPS
          </button>
          <div className="h-3 w-px bg-studio-800 mx-0.5" />
          <button
            onClick={() => setResolution(resolution === "1080p" ? "4K" : "1080p")}
            className="px-2 py-0.5 rounded text-indigo-400 hover:text-indigo-300 font-medium"
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
