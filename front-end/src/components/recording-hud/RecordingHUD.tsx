import React, { useEffect, useState, useCallback } from "react";
import {
  Monitor,
  AppWindow,
  Camera,
  Mic,
  Volume2,
  Circle,
  Square,
  Pause,
  Play,
  Layers,
  AlertTriangle,
  Loader2,
  Settings,
} from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { useRecording } from "../../hooks/useRecording";
import { api } from "../../lib/ipc";
import {
  CaptureSource,
  CameraDevice,
  AudioDevice,
  PermissionBundle,
  PermissionState,
} from "../../lib/types";

const ZERO_PERMISSIONS: PermissionBundle = {
  screenRecording: "unknown",
  camera: "unknown",
  microphone: "unknown",
};

const openSystemSettings = async (
  pane?: "ScreenCapture" | "Camera" | "Microphone",
): Promise<void> => {
  try {
    await api.openSystemPrivacySettings(pane);
  } catch (err) {
    // Synthetic / unrecognised environment — caller is expected to surface
    // inline instructions, so silently swallow here.
    console.warn("[Permissions] openSystemPrivacySettings not available:", err);
  }
};

const PermissionPill: React.FC<{
  label: string;
  state: PermissionState;
  showState?: boolean;
  className?: string;
  onClick?: () => void;
}> = ({ label, state, showState, className, onClick }) => {
  const color =
    state === "authorized"
      ? "text-emerald-400"
      : state === "denied" || state === "restricted"
      ? "text-rose-400 font-semibold"
      : state === "notDetermined"
      ? "text-amber-300"
      : "text-studio-400";
  return (
    <span
      onClick={onClick}
      className={
        color +
        (onClick ? " cursor-pointer hover:underline" : "") +
        (className ? " " + className : "")
      }
      title={`${label}: ${state}${onClick ? " (click to open settings)" : ""}`}
    >
      {showState ? `${label}: ${state}` : label}
    </span>
  );
};

export const RecordingHUD: React.FC<{
  onOpenStudio?: () => void;
}> = ({ onOpenStudio }) => {
  const settings = useSettingsStore();
  const {
    sessionState,
    elapsedMs,
    startRecording,
    pauseRecording,
    resumeRecording,
    stopRecording,
  } = useRecording();

  const [sources, setSources] = useState<CaptureSource[]>([]);
  const [cameras, setCameras] = useState<CameraDevice[]>([]);
  const [mics, setMics] = useState<AudioDevice[]>([]);
  const [permissions, setPermissions] = useState<PermissionBundle>(ZERO_PERMISSIONS);
  const [showSourcePicker, setShowSourcePicker] = useState(false);
  const [showDevicePicker, setShowDevicePicker] = useState(false);
  const [permissionsRequested, setPermissionsRequested] = useState(false);
  const [showPermissionHelp, setShowPermissionHelp] = useState(false);

  // Refresh permission state and re-request any not-yet-determined ones. The
  // initial call also kicks off the OS prompt; later calls (e.g. after the
  // tab regains focus) are read-only to avoid spamming the user.
  const refreshPermissions = useCallback(
    async (reRequest: boolean): Promise<PermissionBundle> => {
      try {
        const next = reRequest
          ? await api.requestCapturePermissions()
          : await api.getPermissionStatus();
        setPermissions(next);
        return next;
      } catch (err) {
        console.warn("[Permissions] refresh failed:", err);
        return ZERO_PERMISSIONS;
      }
    },
    [],
  );

  useEffect(() => {
    let mounted = true;

    // Concurrently load available sources and devices, then reconcile selections once
    // with both sources and devices to prevent race conditions where one overwrites
    // the other with empty data.
    Promise.all([api.listCaptureSources(), api.listDevices()])
      .then(([loadedSources, loadedDevices]) => {
        if (!mounted) return;
        setSources(loadedSources);
        setCameras(loadedDevices.cameras);
        setMics(loadedDevices.mics);
        settings.reconcileSelections(
          loadedSources,
          loadedDevices.cameras,
          loadedDevices.mics,
        );

        if (!permissionsRequested) {
          setPermissionsRequested(true);
          refreshPermissions(false);
        }
      })
      .catch((err) => {
        console.error("[RecordingHUD] Failed to enumerate sources or devices:", err);
        if (!mounted) return;
        setSources([]);
        setCameras([]);
        setMics([]);
        settings.reconcileSelections([], [], []);
      });

    refreshPermissions(false);

    return () => {
      mounted = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Refresh permissions whenever the tab/window regains focus so the user is
  // not stuck with stale state after visiting System Settings.
  useEffect(() => {
    const onFocus = () => {
      refreshPermissions(false);
      // Re-enumerate sources and devices when window regains focus in case permissions
      // changed in System Settings (e.g. user allowed Screen Recording, Camera, or Mic)
      Promise.all([api.listCaptureSources(), api.listDevices()])
        .then(([loadedSources, loadedDevices]) => {
          setSources(loadedSources);
          setCameras(loadedDevices.cameras);
          setMics(loadedDevices.mics);
          settings.reconcileSelections(
            loadedSources,
            loadedDevices.cameras,
            loadedDevices.mics,
          );
        })
        .catch(console.error);
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    return () => {
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
    };
  }, [refreshPermissions]);

  const formatElapsed = (ms: number) => {
    const totalSecs = Math.floor(ms / 1000);
    const m = Math.floor(totalSecs / 60).toString().padStart(2, "0");
    const s = (totalSecs % 60).toString().padStart(2, "0");
    const centis = Math.floor((ms % 1000) / 10).toString().padStart(2, "0");
    return `${m}:${s}.${centis}`;
  };

  const selectedSource = sources.find((s) => s.id === settings.selectedSourceId);
  const isRecording = sessionState === "recording";
  const isPaused = sessionState === "paused";
  const isTransitioning = sessionState === "preparing" || sessionState === "stopping";

  const isScreenBlocked =
    permissions.screenRecording === "denied" || permissions.screenRecording === "restricted";
  const isCameraBlocked =
    permissions.camera === "denied" || permissions.camera === "restricted";
  const isMicBlocked =
    permissions.microphone === "denied" || permissions.microphone === "restricted";

  const screenReady = permissions.screenRecording === "authorized";
  const permissionPending = !screenReady && !isScreenBlocked;

  const anyPermissionBlocked = isScreenBlocked || isCameraBlocked || isMicBlocked;

  const canStart =
    !isTransitioning &&
    settings.selectionsReady &&
    // A truthy stale ID is not enough; it must match a source from the latest
    // native enumeration response.
    !!selectedSource &&
    screenReady;

  const startDisabledReason = (() => {
    if (!settings.selectionsReady) return "Loading capture sources…";
    if (!selectedSource) return "Pick a valid capture source before recording.";
    if (isTransitioning) {
      return sessionState === "preparing" ? "Starting recording…" : "Stopping recording…";
    }
    if (permissionPending) return "Waiting for permission prompts to resolve…";
    if (isScreenBlocked) {
      return permissions.screenRecording === "denied"
        ? "Screen Recording permission is denied. Open System Settings → Privacy & Security → Screen Recording to grant it."
        : "Screen Recording permission is restricted by system policy.";
    }
    return "Record";
  })();

  const openSettingsWithHelp = (pane?: "ScreenCapture" | "Camera" | "Microphone") => {
    setShowPermissionHelp(true);
    void openSystemSettings(pane);
  };

  // Defensive commit: only accept a dropdown choice if the ID is in the
  // freshly-loaded list. Otherwise re-run reconciliation.
  const commitSource = (id: string) => {
    if (sources.some((s) => s.id === id)) {
      settings.setSelectedSourceId(id);
    } else {
      settings.reconcileSelections(sources, cameras, mics);
    }
    setShowSourcePicker(false);
  };

  const commitCamera = (id: string) => {
    if (cameras.some((c) => c.id === id)) {
      settings.setSelectedCameraId(id);
    } else {
      settings.reconcileSelections(sources, cameras, mics);
    }
    setShowDevicePicker(false);
  };

  const commitMic = (id: string) => {
    if (mics.some((m) => m.id === id)) {
      settings.setSelectedMicId(id);
    } else {
      settings.reconcileSelections(sources, cameras, mics);
    }
    setShowDevicePicker(false);
  };

  const handleStart = () => {
    if (!canStart) return;
    void startRecording();
  };

  return (
    <header className="h-16 border-b border-studio-800 bg-studio-900/90 backdrop-blur-md px-6 flex items-center justify-between select-none">
      {/* Brand & Mode */}
      <div className="flex items-center space-x-3">
        <div className="flex items-center justify-center w-8 h-8 rounded-lg bg-indigo-600/20 border border-indigo-500/30 text-indigo-400 font-bold text-lg">
          ▲
        </div>
        <div>
          <span className="font-semibold text-white tracking-wide">AeroShoot</span>
          <span className="text-xs ml-1.5 px-1.5 py-0.5 rounded bg-indigo-500/20 text-indigo-300 font-mono font-medium">
            AI Studio
          </span>
        </div>
      </div>

      {/* Center Controls: Source Selection & Capture Settings */}
      <div className="flex items-center space-x-2">
        {/* Source Selector Button */}
        <div className="relative">
          <button
            disabled={isRecording || isPaused}
            onClick={() => setShowSourcePicker(!showSourcePicker)}
            className="flex items-center space-x-2 px-3 py-1.5 rounded-md bg-studio-800/80 hover:bg-studio-700 border border-studio-700 text-sm font-medium text-studio-100 disabled:opacity-50 transition-colors"
          >
            {selectedSource?.sourceType === "window" ? (
              <AppWindow className="w-4 h-4 text-indigo-400" />
            ) : selectedSource ? (
              <Monitor className="w-4 h-4 text-emerald-400" />
            ) : !settings.selectionsReady ? (
              <Loader2 className="w-4 h-4 text-studio-400 animate-spin" />
            ) : (
              <Monitor className="w-4 h-4 text-studio-500" />
            )}
            <span className="max-w-[160px] truncate">
              {settings.selectionsReady
                ? selectedSource
                  ? selectedSource.name
                  : "Select Source"
                : "Loading sources…"}
            </span>
          </button>

          {/* Source Dropdown Popover */}
          {showSourcePicker && settings.selectionsReady && (
            <div className="absolute top-12 left-0 w-80 bg-studio-900 border border-studio-700 rounded-xl shadow-2xl p-2 z-50 animate-in fade-in zoom-in-95">
              <div className="text-xs font-semibold uppercase text-studio-400 px-3 py-1.5 flex items-center justify-between">
                <span>Displays & Windows</span>
                {isScreenBlocked && (
                  <span className="text-[10px] text-rose-400 font-semibold lowercase">
                    {permissions.screenRecording}
                  </span>
                )}
              </div>

              {isScreenBlocked && (
                <div className="m-2 p-2.5 rounded-lg bg-rose-950/60 border border-rose-800/60 text-xs text-rose-200 space-y-1.5">
                  <div className="flex items-center space-x-1.5 font-semibold text-rose-300">
                    <AlertTriangle className="w-3.5 h-3.5 text-rose-400 shrink-0" />
                    <span>Screen Recording Blocked</span>
                  </div>
                  <p className="text-[11px] text-rose-300/90 leading-snug">
                    macOS requires Screen Recording permission to list and capture displays. Enable AeroShoot in System Settings → Privacy & Security → Screen Recording.
                  </p>
                  <button
                    onClick={() => openSettingsWithHelp("ScreenCapture")}
                    className="inline-flex items-center space-x-1 px-2 py-1 rounded bg-rose-900/60 hover:bg-rose-800 text-rose-100 text-[10px] font-medium transition-colors"
                  >
                    <Settings className="w-3 h-3" />
                    <span>Open Screen Recording Settings</span>
                  </button>
                </div>
              )}

              <div className="max-h-64 overflow-y-auto space-y-1">
                {sources.length === 0 ? (
                  <div className="px-3 py-4 text-xs text-studio-500">
                    {isScreenBlocked
                      ? "Sources unavailable until Screen Recording permission is granted."
                      : "No sources were reported by the native bridge."}
                  </div>
                ) : (
                  sources.map((src) => (
                    <button
                      key={src.id}
                      onClick={() => commitSource(src.id)}
                      className={`w-full flex items-center space-x-3 px-3 py-2 rounded-lg text-left text-xs transition-colors ${
                        settings.selectedSourceId === src.id
                          ? "bg-indigo-600/20 text-indigo-200 border border-indigo-500/30"
                          : "hover:bg-studio-800 text-studio-300"
                      }`}
                    >
                      {src.sourceType === "window" ? (
                        <AppWindow className="w-4 h-4 shrink-0 text-indigo-400" />
                      ) : (
                        <Monitor className="w-4 h-4 shrink-0 text-emerald-400" />
                      )}
                      <div className="truncate">
                        <div className="font-medium text-white truncate">{src.name}</div>
                        <div className="text-[10px] text-studio-400">
                          {src.width} × {src.height}
                        </div>
                      </div>
                    </button>
                  ))
                )}
              </div>
            </div>
          )}
        </div>

        {/* Camera Toggle & Picker */}
        <button
          onClick={() =>
            settings.updateCameraBubble({ enabled: !settings.cameraBubble.enabled })
          }
          className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-md border text-sm transition-colors ${
            settings.cameraBubble.enabled
              ? "bg-indigo-600/20 border-indigo-500/40 text-indigo-300"
              : "bg-studio-800/40 border-studio-700 text-studio-400"
          }`}
          title="Toggle Webcam Overlay"
        >
          <Camera className="w-4 h-4" />
          <span className="text-xs font-medium">Camera</span>
        </button>

        {/* Audio Toggles */}
        <button
          onClick={() => settings.setCaptureSystemAudio(!settings.captureSystemAudio)}
          className={`flex items-center space-x-1.5 px-3 py-1.5 rounded-md border text-sm transition-colors ${
            settings.captureSystemAudio
              ? "bg-emerald-600/20 border-emerald-500/40 text-emerald-300"
              : "bg-studio-800/40 border-studio-700 text-studio-400"
          }`}
          title="Toggle System Audio Loopback"
        >
          <Volume2 className="w-4 h-4" />
          <span className="text-xs font-medium">Sys Audio</span>
        </button>

        {/* Mic Toggle & Device Picker */}
        <div className="relative">
          <button
            onClick={() => setShowDevicePicker(!showDevicePicker)}
            className="flex items-center space-x-1.5 px-3 py-1.5 rounded-md border border-studio-700 bg-studio-800/80 hover:bg-studio-700 text-studio-200 text-sm transition-colors"
            title="Configure Audio and Camera Inputs"
          >
            <Mic className="w-4 h-4 text-amber-400" />
            <span className="text-xs font-medium">Devices</span>
          </button>

          {showDevicePicker && (
            <div className="absolute top-12 left-0 w-72 bg-studio-900 border border-studio-700 rounded-xl shadow-2xl p-3 z-50 space-y-3 animate-in fade-in zoom-in-95">
              <div>
                <div className="text-[11px] font-semibold uppercase text-studio-400 mb-1.5 flex items-center justify-between">
                  <div className="flex items-center space-x-1.5">
                    <Mic className="w-3.5 h-3.5 text-amber-400" />
                    <span>Microphones</span>
                  </div>
                  {isMicBlocked && (
                    <span className="text-[10px] text-rose-400 font-semibold lowercase">
                      {permissions.microphone}
                    </span>
                  )}
                </div>

                {isMicBlocked && (
                  <div className="mb-2 p-2 rounded-lg bg-rose-950/50 border border-rose-800/50 text-[11px] text-rose-200 space-y-1">
                    <p className="leading-tight">Microphone access is blocked in macOS Settings.</p>
                    <button
                      onClick={() => openSettingsWithHelp("Microphone")}
                      className="inline-flex items-center space-x-1 px-1.5 py-0.5 rounded bg-rose-900/60 hover:bg-rose-800 text-rose-100 text-[10px] font-medium transition-colors"
                    >
                      <Settings className="w-3 h-3" />
                      <span>Open Microphone Settings</span>
                    </button>
                  </div>
                )}

                <div className="space-y-1">
                  {mics.length === 0 ? (
                    <div className="px-2.5 py-1.5 text-xs text-studio-500">
                      {isMicBlocked
                        ? "Microphones unavailable (permission denied)."
                        : "No microphones detected."}
                    </div>
                  ) : (
                    mics.map((m) => (
                      <button
                        key={m.id}
                        onClick={() => commitMic(m.id)}
                        className={`w-full text-left px-2.5 py-1.5 rounded-md text-xs truncate transition-colors ${
                          settings.selectedMicId === m.id
                            ? "bg-indigo-600/20 text-indigo-200 border border-indigo-500/30"
                            : "hover:bg-studio-800 text-studio-300"
                        }`}
                      >
                        {m.name}
                      </button>
                    ))
                  )}
                </div>
              </div>

              <div className="border-t border-studio-800 pt-2">
                <div className="text-[11px] font-semibold uppercase text-studio-400 mb-1.5 flex items-center justify-between">
                  <div className="flex items-center space-x-1.5">
                    <Camera className="w-3.5 h-3.5 text-indigo-400" />
                    <span>Cameras</span>
                  </div>
                  {isCameraBlocked && (
                    <span className="text-[10px] text-rose-400 font-semibold lowercase">
                      {permissions.camera}
                    </span>
                  )}
                </div>

                {isCameraBlocked && (
                  <div className="mb-2 p-2 rounded-lg bg-rose-950/50 border border-rose-800/50 text-[11px] text-rose-200 space-y-1">
                    <p className="leading-tight">Camera access is blocked in macOS Settings.</p>
                    <button
                      onClick={() => openSettingsWithHelp("Camera")}
                      className="inline-flex items-center space-x-1 px-1.5 py-0.5 rounded bg-rose-900/60 hover:bg-rose-800 text-rose-100 text-[10px] font-medium transition-colors"
                    >
                      <Settings className="w-3 h-3" />
                      <span>Open Camera Settings</span>
                    </button>
                  </div>
                )}

                <div className="space-y-1">
                  {cameras.length === 0 ? (
                    <div className="px-2.5 py-1.5 text-xs text-studio-500">
                      {isCameraBlocked
                        ? "Cameras unavailable (permission denied)."
                        : "No cameras detected."}
                    </div>
                  ) : (
                    cameras.map((c) => (
                      <button
                        key={c.id}
                        onClick={() => commitCamera(c.id)}
                        className={`w-full text-left px-2.5 py-1.5 rounded-md text-xs truncate transition-colors ${
                          settings.selectedCameraId === c.id
                            ? "bg-indigo-600/20 text-indigo-200 border border-indigo-500/30"
                            : "hover:bg-studio-800 text-studio-300"
                        }`}
                      >
                        {c.name}
                      </button>
                    ))
                  )}
                </div>
              </div>
            </div>
          )}
        </div>

        {/* Quality preset badge */}
        <div className="flex items-center bg-studio-800 border border-studio-700 rounded-md p-0.5 text-xs">
          <button
            onClick={() => settings.setFps(30)}
            className={`px-2 py-1 rounded font-mono ${
              settings.fps === 30 ? "bg-studio-700 text-white font-semibold" : "text-studio-400"
            }`}
          >
            30 FPS
          </button>
          <button
            onClick={() => settings.setFps(60)}
            className={`px-2 py-1 rounded font-mono ${
              settings.fps === 60 ? "bg-studio-700 text-white font-semibold" : "text-studio-400"
            }`}
          >
            60 FPS
          </button>
        </div>
      </div>

      {/* Right Side: Recording State, Timer & Trigger Buttons */}
      <div className="flex items-center space-x-3">
        {/* Permission status: notDetermined spinner, denied/restricted warning
            pill group with actionable links, and explicit hints. */}
        {permissionPending && (
          <div
            className="flex items-center space-x-1.5 px-2.5 py-1 rounded-md bg-studio-800/60 border border-studio-700/60 text-xs font-mono"
            title="Permission prompts are still resolving"
          >
            <Loader2 className="w-3.5 h-3.5 text-amber-300 animate-spin shrink-0" />
            <span className="text-studio-300">Requesting permissions…</span>
          </div>
        )}

        {anyPermissionBlocked && (
          <div
            className="flex items-center space-x-2 px-2.5 py-1 rounded-md bg-amber-950/40 border border-amber-800/40 text-xs font-mono"
            title="Actionable permission warning"
          >
            <AlertTriangle className="w-3.5 h-3.5 text-amber-400 shrink-0" />
            <PermissionPill
              label="Screen"
              state={permissions.screenRecording}
              onClick={() => openSettingsWithHelp("ScreenCapture")}
            />
            <span className="text-studio-500">•</span>
            <PermissionPill
              label="Cam"
              state={permissions.camera}
              onClick={() => openSettingsWithHelp("Camera")}
            />
            <span className="text-studio-500">•</span>
            <PermissionPill
              label="Mic"
              state={permissions.microphone}
              onClick={() => openSettingsWithHelp("Microphone")}
            />
            <button
              onClick={() => {
                const pane = isScreenBlocked
                  ? "ScreenCapture"
                  : isCameraBlocked
                  ? "Camera"
                  : "Microphone";
                openSettingsWithHelp(pane);
              }}
              className="ml-1 flex items-center space-x-1 px-1.5 py-0.5 rounded bg-amber-800/40 hover:bg-amber-700/50 text-amber-200 border border-amber-700/60 transition-colors"
              title="Open macOS Privacy & Security settings"
            >
              <Settings className="w-3 h-3" />
              <span>Settings</span>
            </button>
          </div>
        )}

        {/* Actionable guidance banner for blocked permissions */}
        {isScreenBlocked ? (
          <div
            className="hidden xl:flex items-center space-x-1.5 px-2.5 py-1 rounded-md bg-rose-950/40 border border-rose-800/40 text-xs text-rose-200"
            title="Screen Recording is a system-level permission and must be granted from System Settings."
          >
            <AlertTriangle className="w-3.5 h-3.5 text-rose-400 shrink-0" />
            <span>
              Screen Recording {permissions.screenRecording}.{" "}
              <button
                onClick={() => openSettingsWithHelp("ScreenCapture")}
                className="underline hover:text-white font-medium"
              >
                Open Settings
              </button>
            </span>
          </div>
        ) : isCameraBlocked ? (
          <div
            className="hidden xl:flex items-center space-x-1.5 px-2.5 py-1 rounded-md bg-amber-950/40 border border-amber-800/40 text-xs text-amber-200"
            title="Camera permission is denied. Recording can continue without the webcam bubble."
          >
            <AlertTriangle className="w-3.5 h-3.5 text-amber-400 shrink-0" />
            <span>
              Camera {permissions.camera}.{" "}
              <button
                onClick={() => openSettingsWithHelp("Camera")}
                className="underline hover:text-white font-medium"
              >
                Open Settings
              </button>
            </span>
          </div>
        ) : isMicBlocked ? (
          <div
            className="hidden xl:flex items-center space-x-1.5 px-2.5 py-1 rounded-md bg-amber-950/40 border border-amber-800/40 text-xs text-amber-200"
            title="Microphone permission is required for microphone audio capture."
          >
            <AlertTriangle className="w-3.5 h-3.5 text-amber-400 shrink-0" />
            <span>
              Microphone {permissions.microphone}.{" "}
              <button
                onClick={() => openSettingsWithHelp("Microphone")}
                className="underline hover:text-white font-medium"
              >
                Open Settings
              </button>
            </span>
          </div>
        ) : null}

        {/* Inline validation feedback when the user is hovering the record
            button. Surfaced as the button `title` and a small caption beneath
            the controls row when the record action is blocked. */}
        {!isRecording && !isPaused && !canStart && (
          <span
            className="text-[11px] text-amber-300 max-w-[220px] truncate"
            title={startDisabledReason}
          >
            {startDisabledReason}
          </span>
        )}

        {/* Status indicator & Timer */}
        <div className="flex items-center space-x-2.5 px-3 py-1.5 rounded-md bg-studio-850 border border-studio-700 font-mono text-sm">
          {isRecording && (
            <span className="relative flex h-2.5 w-2.5">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-rose-400 opacity-75"></span>
              <span className="relative inline-flex rounded-full h-2.5 w-2.5 bg-rose-500"></span>
            </span>
          )}
          {isPaused && <span className="h-2.5 w-2.5 rounded-full bg-amber-400"></span>}
          {!isRecording && !isPaused && (
            <span className="h-2.5 w-2.5 rounded-full bg-studio-500"></span>
          )}
          <span
            className={`font-semibold ${
              isRecording ? "text-rose-400" : isPaused ? "text-amber-400" : "text-studio-300"
            }`}
          >
            {formatElapsed(elapsedMs)}
          </span>
        </div>

        {/* Start / Pause / Stop Actions */}
        {!isRecording && !isPaused ? (
          <button
            onClick={handleStart}
            disabled={!canStart}
            title={startDisabledReason}
            className="flex items-center space-x-2 px-4 py-2 rounded-lg bg-rose-600 hover:bg-rose-500 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-rose-600 disabled:hover:scale-100 text-white text-sm font-semibold shadow-lg shadow-rose-900/30 transition-all hover:scale-105 active:scale-95"
          >
            <Circle className="w-4 h-4 fill-white" />
            <span>Record</span>
          </button>
        ) : (
          <div className="flex items-center space-x-2">
            {isRecording ? (
              <button
                onClick={pauseRecording}
                className="p-2 rounded-lg bg-studio-800 hover:bg-studio-700 border border-studio-700 text-amber-400 transition-colors"
                title="Pause Capture"
              >
                <Pause className="w-4 h-4" />
              </button>
            ) : (
              <button
                onClick={resumeRecording}
                className="p-2 rounded-lg bg-studio-800 hover:bg-studio-700 border border-studio-700 text-emerald-400 transition-colors"
                title="Resume Capture"
              >
                <Play className="w-4 h-4 fill-current" />
              </button>
            )}

            <button
              onClick={stopRecording}
              className="flex items-center space-x-1.5 px-3.5 py-2 rounded-lg bg-studio-800 hover:bg-rose-950/40 border border-rose-700/50 text-rose-300 text-sm font-semibold transition-colors"
              title="Stop and Open in Studio"
            >
              <Square className="w-4 h-4 fill-rose-500 text-rose-500" />
              <span>Stop & Edit</span>
            </button>
          </div>
        )}

        {/* Studio View Button */}
        {onOpenStudio && (
          <button
            onClick={onOpenStudio}
            className="p-2 rounded-lg bg-studio-800 hover:bg-studio-700 border border-studio-700 text-studio-200"
            title="Open Timeline Studio"
          >
            <Layers className="w-4 h-4" />
          </button>
        )}
      </div>

      {/* Actionable permissions help panel */}
      {showPermissionHelp && (
        <div className="absolute top-16 right-6 w-96 z-50 bg-studio-900 border border-amber-800/60 rounded-xl shadow-2xl p-4 text-xs text-studio-200 space-y-3 animate-in fade-in zoom-in-95">
          <div className="flex items-center justify-between border-b border-studio-800 pb-2">
            <div className="font-semibold text-amber-300 flex items-center space-x-1.5">
              <Settings className="w-4 h-4 text-amber-400" />
              <span>macOS Privacy & Security Settings</span>
            </div>
            <button
              onClick={() => setShowPermissionHelp(false)}
              className="text-xs text-studio-400 hover:text-studio-200 px-1"
            >
              ✕
            </button>
          </div>

          <p className="text-[11px] text-studio-300 leading-relaxed">
            AeroShoot requires macOS system permissions to record your screen, camera, and microphone.
          </p>

          <div className="space-y-2 text-[11px]">
            {/* Screen Recording */}
            <div className="p-2.5 rounded-lg bg-studio-850 border border-studio-800 flex items-center justify-between">
              <div className="space-y-0.5">
                <div className="font-medium text-white flex items-center space-x-1.5">
                  <Monitor className="w-3.5 h-3.5 text-emerald-400" />
                  <span>Screen Recording</span>
                </div>
                <div className="text-[10px] text-studio-400">
                  Required to record displays and windows.
                </div>
              </div>
              <div className="flex items-center space-x-2">
                <PermissionPill label="Screen" state={permissions.screenRecording} />
                <button
                  onClick={() => void openSystemSettings("ScreenCapture")}
                  className="px-2 py-1 rounded bg-indigo-600/30 hover:bg-indigo-600/50 text-indigo-200 border border-indigo-500/40 text-[10px] font-medium transition-colors"
                >
                  Open
                </button>
              </div>
            </div>

            {/* Camera */}
            <div className="p-2.5 rounded-lg bg-studio-850 border border-studio-800 flex items-center justify-between">
              <div className="space-y-0.5">
                <div className="font-medium text-white flex items-center space-x-1.5">
                  <Camera className="w-3.5 h-3.5 text-indigo-400" />
                  <span>Camera</span>
                </div>
                <div className="text-[10px] text-studio-400">
                  Required for webcam bubble overlay.
                </div>
              </div>
              <div className="flex items-center space-x-2">
                <PermissionPill label="Cam" state={permissions.camera} />
                <button
                  onClick={() => void openSystemSettings("Camera")}
                  className="px-2 py-1 rounded bg-indigo-600/30 hover:bg-indigo-600/50 text-indigo-200 border border-indigo-500/40 text-[10px] font-medium transition-colors"
                >
                  Open
                </button>
              </div>
            </div>

            {/* Microphone */}
            <div className="p-2.5 rounded-lg bg-studio-850 border border-studio-800 flex items-center justify-between">
              <div className="space-y-0.5">
                <div className="font-medium text-white flex items-center space-x-1.5">
                  <Mic className="w-3.5 h-3.5 text-amber-400" />
                  <span>Microphone</span>
                </div>
                <div className="text-[10px] text-studio-400">
                  Required for voice audio commentary.
                </div>
              </div>
              <div className="flex items-center space-x-2">
                <PermissionPill label="Mic" state={permissions.microphone} />
                <button
                  onClick={() => void openSystemSettings("Microphone")}
                  className="px-2 py-1 rounded bg-indigo-600/30 hover:bg-indigo-600/50 text-indigo-200 border border-indigo-500/40 text-[10px] font-medium transition-colors"
                >
                  Open
                </button>
              </div>
            </div>
          </div>

          <div className="text-[10px] text-studio-400 bg-studio-800/40 p-2 rounded-md space-y-1">
            <p>
              💡 After toggling permissions in System Settings, return to AeroShoot. The status refreshes automatically on window focus.
            </p>
            {isScreenBlocked && (
              <p className="text-amber-300 font-medium">
                Note: macOS may require restarting the application after granting Screen Recording permission.
              </p>
            )}
          </div>

          <div className="flex justify-between items-center pt-1 border-t border-studio-800">
            <button
              onClick={() => {
                void refreshPermissions(true);
              }}
              className="text-[11px] text-indigo-400 hover:text-indigo-300 font-medium"
            >
              Re-check Permissions
            </button>
            <button
              onClick={() => setShowPermissionHelp(false)}
              className="px-2.5 py-1 rounded bg-studio-800 hover:bg-studio-700 text-[11px] text-studio-200 transition-colors"
            >
              Close
            </button>
          </div>
        </div>
      )}
    </header>
  );
};
