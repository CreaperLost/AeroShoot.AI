import React, { useState, useRef, useEffect, useLayoutEffect } from "react";
import {
  Monitor,
  Camera,
  CameraOff,
  Mic,
  Volume2,
  VolumeX,
  ChevronDown,
  Check,
} from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { CaptureSource, CameraDevice, AudioDevice, PermissionBundle } from "../../lib/types";
import { MicGainSlider } from "./MicGainSlider";
import { api } from "../../lib/ipc";

interface DeviceControlDeckProps {
  sources: CaptureSource[];
  cameras: CameraDevice[];
  mics: AudioDevice[];
  disabled?: boolean;
  onDropdownOpenChange?: (open: boolean) => void;
  onRequestCameraPermission?: () => Promise<void>;
  onRequestMicrophonePermission?: () => Promise<void>;
  permissions: PermissionBundle;
  needsScreenPermission: boolean;
  onRequestScreenPermission?: () => Promise<void>;
}

export const DeviceControlDeck: React.FC<DeviceControlDeckProps> = ({
  sources,
  cameras,
  mics,
  disabled = false,
  onDropdownOpenChange,
  onRequestCameraPermission,
  onRequestMicrophonePermission,
  permissions,
  needsScreenPermission,
  onRequestScreenPermission,
}) => {
  const settings = useSettingsStore();

  const [openDropdown, setOpenDropdown] = useState<"source" | "mic" | "camera" | null>(null);
  const [micPeakDb, setMicPeakDb] = useState<number | null>(null);
  const [systemPeakDb, setSystemPeakDb] = useState<number | null>(null);

  const containerRef = useRef<HTMLDivElement | null>(null);

  // Close dropdown on outside click
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(event.target as Node)) {
        setOpenDropdown(null);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  useEffect(() => {
    if (!openDropdown) return;
    window.requestAnimationFrame(() => {
      containerRef.current
        ?.querySelector<HTMLElement>("[role='menu'] [role^='menuitem']")
        ?.focus();
    });
  }, [openDropdown]);

  const handleMenuKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (!openDropdown || !["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const items = Array.from(
      containerRef.current?.querySelectorAll<HTMLElement>("[role='menu'] [role^='menuitem']:not([disabled])") ?? [],
    );
    if (items.length === 0) return;
    event.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLElement);
    const next = event.key === "Home" ? 0
      : event.key === "End" ? items.length - 1
      : event.key === "ArrowUp" ? (current <= 0 ? items.length - 1 : current - 1)
      : (current + 1) % items.length;
    items[next]?.focus();
  };

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpenDropdown(null);
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, []);

  useLayoutEffect(() => {
    onDropdownOpenChange?.(openDropdown !== null);
    return () => onDropdownOpenChange?.(false);
  }, [openDropdown, onDropdownOpenChange]);

  useEffect(() => {
    if ((!settings.captureSystemAudio && !settings.selectedMicId) || disabled) {
      setSystemPeakDb(null);
      setMicPeakDb(null);
      return;
    }
    let active = true;
    const poll = () => void api.capturePreviewAudioLevels().then((levels) => {
      if (active) {
        setSystemPeakDb(levels.systemAudioPeakDb ?? null);
        setMicPeakDb(levels.micPeakDb ?? null);
      }
    }).catch(() => {
      if (active) { setSystemPeakDb(null); setMicPeakDb(null); }
    });
    poll();
    const timer = window.setInterval(poll, 150);
    return () => { active = false; window.clearInterval(timer); };
  }, [settings.captureSystemAudio, settings.selectedMicId, disabled]);

  const displays = sources.filter((s) => s.sourceType === "display");
  const selectedSource = displays.find((s) => s.id === settings.selectedSourceId);
  const selectedMic = mics.find((m) => m.id === settings.selectedMicId);
  const selectedCamera = cameras.find((c) => c.id === settings.selectedCameraId) ?? cameras[0];
  const screenPermissionProblem = needsScreenPermission && permissions.screenRecording !== "authorized";
  const cameraPermissionProblem = settings.cameraBubble.enabled && permissions.camera !== "authorized";
  const micPermissionProblem = Boolean(settings.selectedMicId) && permissions.microphone !== "authorized";

  const permissionHint = (kind: "screen" | "camera" | "microphone") => {
    const state = kind === "screen" ? permissions.screenRecording : permissions[kind];
    if (state === "denied" || state === "restricted") return "Permission blocked — open macOS Settings";
    return "Permission required — activate to allow";
  };

  return (
    <div
      ref={containerRef}
      onKeyDown={handleMenuKeyDown}
      className="relative flex w-full flex-col gap-2 text-xs select-none"
    >
      <div className="device-selectors-grid">
        {/* 1. PHYSICAL DISPLAY SELECTOR */}
        <div className="relative min-w-0">
          <button
            type="button"
            disabled={disabled}
            aria-label={`Display source: ${selectedSource?.name ?? "No Screen"}`}
            aria-haspopup="menu"
            aria-expanded={openDropdown === "source"}
            aria-controls="display-source-menu"
            aria-describedby={screenPermissionProblem ? "screen-permission-status" : undefined}
            onClick={() => setOpenDropdown(openDropdown === "source" ? null : "source")}
            className={`device-control-card flex w-full min-w-0 items-center space-x-2.5 px-3 py-2 rounded-xl border transition-all ${
              openDropdown === "source"
                ? "bg-studio-800 border-indigo-500/80 text-white shadow-lg shadow-indigo-500/10"
                : "bg-studio-850/80 hover:bg-studio-800 border-studio-750 text-studio-200"
            }`}
          >
            <div className="p-1 rounded-lg bg-indigo-500/15 text-indigo-400">
              <Monitor className="w-4 h-4" />
            </div>
            <div className="flex flex-col text-left">
              <span className="text-[10px] uppercase font-semibold tracking-wider text-studio-400">
                Display
              </span>
              <span className="font-medium text-white max-w-[170px] truncate">
                {selectedSource ? selectedSource.name : "No Display"}
              </span>
            </div>
            {selectedSource && (
              <span className="device-control-detail text-[10px] font-mono px-1.5 py-0.5 rounded bg-studio-800 border border-studio-700 text-studio-400">
                {selectedSource.width}×{selectedSource.height}
              </span>
            )}
            <ChevronDown
              className={`w-3.5 h-3.5 text-studio-400 transition-transform ${
                openDropdown === "source" ? "rotate-180" : ""
              }`}
            />
          </button>

          {openDropdown === "source" && (
            <div id="display-source-menu" role="menu" aria-label="Display sources" className="device-dropdown-panel absolute inset-x-0 top-full mt-2bg-studio-900 border border-studio-700/80 rounded-2xl shadow-2xl p-2 z-50 animate-in fade-in zoom-in-95 backdrop-blur-xl">
              <div className="text-[11px] font-semibold text-studio-400 uppercase px-3 py-1.5 border-b border-studio-800 flex justify-between">
                <span>Displays</span>
                <span className="text-[10px] text-studio-500 font-mono">
                  {displays.length} detected
                </span>
              </div>
              <div className="py-1 space-y-0.5 max-h-56 overflow-y-auto">
                <button
                  type="button"
                  role="menuitemradio"
                  aria-checked={settings.selectedSourceId === null}
                  onClick={() => {
                    settings.setSelectedSourceId(null);
                    setOpenDropdown(null);
                  }}
                  className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-left transition-colors ${
                    settings.selectedSourceId === null
                      ? "bg-indigo-600/20 text-indigo-200 border border-indigo-500/30 font-medium"
                      : "text-studio-300 hover:bg-studio-800/80"
                  }`}
                >
                  <div className="flex items-center space-x-2.5 truncate">
                    <VolumeX className="w-4 h-4 text-studio-400 shrink-0" />
                    <div className="truncate">
                      <div className="font-medium text-white">No Screen</div>
                      <div className="text-[10px] text-studio-500">Record only the other enabled sources</div>
                    </div>
                  </div>
                  {settings.selectedSourceId === null && <Check className="w-3.5 h-3.5 text-indigo-400 shrink-0 ml-2" />}
                </button>
                <div className="h-px bg-studio-800 my-1" />
                {displays.length === 0 ? (
                  <div className="px-3 py-3 text-studio-400 text-center">No displays detected</div>
                ) : (
                  displays.map((disp) => (
                    <button
                      key={disp.id}
                      type="button"
                      role="menuitemradio"
                      aria-checked={settings.selectedSourceId === disp.id}
                      onClick={() => {
                        settings.setSelectedSourceId(disp.id);
                        setOpenDropdown(null);
                      }}
                      className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-left transition-colors ${
                        settings.selectedSourceId === disp.id
                          ? "bg-indigo-600/20 text-indigo-200 border border-indigo-500/30 font-medium"
                          : "text-studio-200 hover:bg-studio-800/80"
                      }`}
                    >
                      <div className="flex items-center space-x-2.5 truncate">
                        <Monitor className="w-4 h-4 text-emerald-400 shrink-0" />
                        <div className="truncate">
                          <div className="truncate font-medium">{disp.name}</div>
                          <div className="text-[10px] text-studio-400 font-mono">
                            {disp.width} × {disp.height}
                          </div>
                        </div>
                      </div>
                      {settings.selectedSourceId === disp.id && (
                        <Check className="w-3.5 h-3.5 text-indigo-400 shrink-0 ml-2" />
                      )}
                    </button>
                  ))
                )}
              </div>
            </div>
          )}
          {screenPermissionProblem && <button id="screen-permission-status" type="button" onClick={() => void onRequestScreenPermission?.()} className="source-permission-issue" aria-label={permissionHint("screen")}>{permissionHint("screen")}</button>}
        </div>

        {/* 2. MICROPHONE / AUDIO SELECTOR */}
        <div className="relative min-w-0">
          <button
            type="button"
            disabled={disabled}
            aria-label={`Microphone source: ${selectedMic?.name ?? "No Microphone"}`}
            aria-haspopup="menu"
            aria-expanded={openDropdown === "mic"}
            aria-controls="microphone-source-menu"
            aria-describedby={micPermissionProblem ? "microphone-permission-status" : undefined}
            onClick={() => setOpenDropdown(openDropdown === "mic" ? null : "mic")}
            className={`device-control-card flex w-full min-w-0 items-center space-x-2.5 px-3 py-2 rounded-xl border transition-all ${
              openDropdown === "mic"
                ? "bg-studio-800 border-amber-500/80 text-white shadow-lg shadow-amber-500/10"
                : "bg-studio-850/80 hover:bg-studio-800 border-studio-750 text-studio-200"
            }`}
          >
            <div className="p-1 rounded-lg bg-amber-500/15 text-amber-400 relative">
              <Mic className="w-4 h-4" />
              {/* Live activity dot, derived from MicPreview's peak */}
              {selectedMic && micPeakDb !== null && Number.isFinite(micPeakDb) && micPeakDb > -50 && (
                <span className="absolute -top-0.5 -right-0.5 w-2 h-2 rounded-full bg-emerald-500 animate-pulse" />
              )}
            </div>
            <div className="flex flex-col text-left">
              <span className="text-[10px] uppercase font-semibold tracking-wider text-studio-400">
                Microphone
              </span>
              <span className="font-medium text-white max-w-[150px] truncate">
                {selectedMic ? selectedMic.name : "No Microphone"}
              </span>
            </div>
            {/* Live waveform + gain readout */}
            {selectedMic && (
              <div className="device-control-detail flex items-center space-x-1.5">
                <span className="h-1.5 w-10 overflow-hidden rounded-full bg-studio-800" title={micPeakDb === null ? "Waiting for microphone audio" : `${micPeakDb.toFixed(1)} dBFS`}>
                  <span className={`block h-full rounded-full ${micPeakDb !== null && micPeakDb > -3 ? "bg-rose-400" : "bg-amber-400"}`}
                    style={{ width: `${micPeakDb === null ? 0 : Math.max(0, Math.min(100, ((micPeakDb + 60) / 60) * 100))}%` }} />
                </span>
                <span
                  className={`text-[10px] font-mono ${
                    settings.micGainDb === 0
                      ? "text-studio-400"
                      : settings.micGainDb > 0
                        ? "text-amber-300"
                        : "text-sky-300"
                  }`}
                  title="Applied mic gain"
                >
                  {settings.micGainDb > 0 ? "+" : ""}
                  {settings.micGainDb} dB
                </span>
              </div>
            )}
            <ChevronDown
              className={`w-3.5 h-3.5 text-studio-400 transition-transform ${
                openDropdown === "mic" ? "rotate-180" : ""
              }`}
            />
          </button>

          {openDropdown === "mic" && (
            <div id="microphone-source-menu" role="menu" aria-label="Microphone sources" className="device-dropdown-panel absolute inset-x-0 top-full mt-2bg-studio-900 border border-studio-700/80 rounded-2xl shadow-2xl p-2 z-50 animate-in fade-in zoom-in-95 backdrop-blur-xl">
              <div className="text-[11px] font-semibold text-studio-400 uppercase px-3 py-1.5 border-b border-studio-800 flex justify-between">
                <span>Audio Inputs</span>
                <span className="text-[10px] text-studio-500 font-mono">{mics.length} detected</span>
              </div>
              <div className="py-1 space-y-0.5 max-h-48 overflow-y-auto">
                <button
                  type="button"
                  role="menuitemradio"
                  aria-checked={settings.selectedMicId === null}
                  onClick={() => {
                    settings.setSelectedMicId(null);
                    setOpenDropdown(null);
                  }}
                  className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-left transition-colors ${
                    settings.selectedMicId === null
                      ? "bg-amber-500/20 text-amber-200 border border-amber-500/30 font-medium"
                      : "text-studio-300 hover:bg-studio-800/80"
                  }`}
                >
                  <div className="flex items-center space-x-2.5 truncate">
                    <VolumeX className="w-4 h-4 text-studio-400 shrink-0" />
                    <div className="truncate">
                      <div className="font-medium text-white">No Microphone</div>
                      <div className="text-[10px] text-studio-500">Do not create a mic track</div>
                    </div>
                  </div>
                  {settings.selectedMicId === null && (
                    <Check className="w-3.5 h-3.5 text-amber-400 shrink-0 ml-2" />
                  )}
                </button>
                <div className="h-px bg-studio-800 my-1" />
                {mics.length === 0 ? (
                  <div className="px-3 py-3 text-studio-400 text-center">No microphones detected</div>
                ) : (
                  mics.map((mic) => (
                    <button
                      key={mic.id}
                      type="button"
                      role="menuitemradio"
                      aria-checked={settings.selectedMicId === mic.id}
                      onClick={() => {
                        settings.setSelectedMicId(mic.id);
                        setOpenDropdown(null);
                        void onRequestMicrophonePermission?.();
                      }}
                      className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-left transition-colors ${
                        settings.selectedMicId === mic.id
                          ? "bg-amber-500/20 text-amber-200 border border-amber-500/30 font-medium"
                          : "text-studio-200 hover:bg-studio-800/80"
                      }`}
                    >
                      <div className="flex items-center space-x-2.5 truncate">
                        <Mic className="w-4 h-4 text-amber-400 shrink-0" />
                        <span className="truncate">{mic.name}</span>
                      </div>
                      {settings.selectedMicId === mic.id && (
                        <Check className="w-3.5 h-3.5 text-amber-400 shrink-0 ml-2" />
                      )}
                    </button>
                  ))
                )}
              </div>
              {selectedMic && (
                <div className="border-t border-studio-800 mt-1 pt-3 px-3 pb-2 space-y-3">
                  <div className="flex items-center space-x-3">
                    <span className="h-2.5 w-36 overflow-hidden rounded-full bg-studio-800" title={micPeakDb === null ? "Waiting for native microphone audio" : `${micPeakDb.toFixed(1)} dBFS`}>
                      <span className={`block h-full rounded-full ${micPeakDb !== null && micPeakDb > -3 ? "bg-rose-400" : micPeakDb !== null && micPeakDb > -12 ? "bg-amber-300" : "bg-emerald-400"}`}
                        style={{ width: `${micPeakDb === null ? 0 : Math.max(0, Math.min(100, ((micPeakDb + 60) / 60) * 100))}%` }} />
                    </span>
                    <div className="flex flex-col text-[10px] font-mono leading-tight">
                      <span className="text-studio-400 uppercase">Peak</span>
                      <span
                        className={`text-base font-semibold ${
                          micPeakDb !== null && Number.isFinite(micPeakDb)
                            ? micPeakDb > -3
                              ? "text-rose-400"
                              : micPeakDb > -12
                                ? "text-amber-300"
                                : "text-emerald-300"
                            : "text-studio-500"
                        }`}
                      >
                        {micPeakDb !== null && Number.isFinite(micPeakDb)
                          ? `${micPeakDb.toFixed(1)}`
                          : "—"}
                      </span>
                      <span className="text-studio-500">dBFS</span>
                    </div>
                  </div>
                  <MicGainSlider
                    value={settings.micGainDb}
                    onChange={settings.setMicGainDb}
                    peakDb={micPeakDb}
                    disabled={disabled}
                  />
                  <p className="text-[10px] text-studio-500 leading-snug">
                    Gain is applied to the captured mic track. Values above 0 dB can clip loud inputs.
                  </p>
                </div>
              )}
            </div>
          )}
          {micPermissionProblem && <button id="microphone-permission-status" type="button" onClick={() => void onRequestMicrophonePermission?.()} className="source-permission-issue" aria-label={permissionHint("microphone")}>{permissionHint("microphone")}</button>}
        </div>

        {/* 3. WEBCAMERA SELECTOR (Unified card with 'No Camera' option) */}
        <div className="relative min-w-0">
          <button
            type="button"
            disabled={disabled}
            aria-label={`Camera source: ${settings.cameraBubble.enabled && selectedCamera ? selectedCamera.name : "No Camera"}`}
            aria-haspopup="menu"
            aria-expanded={openDropdown === "camera"}
            aria-controls="camera-source-menu"
            aria-describedby={cameraPermissionProblem ? "camera-permission-status" : undefined}
            onClick={() => setOpenDropdown(openDropdown === "camera" ? null : "camera")}
            className={`device-control-card flex w-full min-w-0 items-center space-x-2.5 px-3 py-2 rounded-xl border transition-all ${
              openDropdown === "camera"
                ? "bg-studio-800 border-indigo-500/80 text-white shadow-lg shadow-indigo-500/10"
                : settings.cameraBubble.enabled
                ? "bg-studio-850/80 hover:bg-studio-800 border-studio-750 text-studio-200"
                : "bg-studio-850/60 hover:bg-studio-800 border-studio-800 text-studio-400"
            }`}
          >
            <div
              className={`p-1 rounded-lg ${
                settings.cameraBubble.enabled
                  ? "bg-indigo-500/20 text-indigo-300"
                  : "bg-studio-800 text-studio-500"
              }`}
            >
              {settings.cameraBubble.enabled ? (
                <Camera className="w-4 h-4" />
              ) : (
                <CameraOff className="w-4 h-4" />
              )}
            </div>
            <div className="flex flex-col text-left">
              <span className="text-[10px] uppercase font-semibold tracking-wider text-studio-400">
                Webcamera
              </span>
              <span
                className={`font-medium max-w-[140px] truncate ${
                  settings.cameraBubble.enabled ? "text-white" : "text-studio-400"
                }`}
              >
                {settings.cameraBubble.enabled && selectedCamera
                  ? selectedCamera.name
                  : "No Camera"}
              </span>
            </div>
            <ChevronDown
              className={`w-3.5 h-3.5 text-studio-400 transition-transform ${
                openDropdown === "camera" ? "rotate-180" : ""
              }`}
            />
          </button>

          {openDropdown === "camera" && (
            <div id="camera-source-menu" role="menu" aria-label="Camera sources" className="device-dropdown-panel absolute inset-x-0 top-full mt-2bg-studio-900 border border-studio-700/80 rounded-2xl shadow-2xl p-2 z-50 animate-in fade-in zoom-in-95 backdrop-blur-xl">
              <div className="text-[11px] font-semibold text-studio-400 uppercase px-3 py-1.5 border-b border-studio-800 flex justify-between">
                <span>Camera Options</span>
                <span className="text-[10px] text-studio-500 font-mono">
                  {cameras.length} available
                </span>
              </div>
              <div className="py-1 space-y-0.5 max-h-56 overflow-y-auto">
                {/* Option 1: No Camera */}
                <button
                  type="button"
                  role="menuitemradio"
                  aria-checked={!settings.cameraBubble.enabled}
                  onClick={() => {
                    settings.updateCameraBubble({ enabled: false });
                    setOpenDropdown(null);
                  }}
                  className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-left transition-colors ${
                    !settings.cameraBubble.enabled
                      ? "bg-indigo-600/20 text-indigo-200 border border-indigo-500/30 font-medium"
                      : "text-studio-300 hover:bg-studio-800/80"
                  }`}
                >
                  <div className="flex items-center space-x-2.5 truncate">
                    <CameraOff className="w-4 h-4 text-studio-400 shrink-0" />
                    <div className="truncate">
                      <div className="font-medium text-white">No Camera</div>
                      <div className="text-[10px] text-studio-500">Disable webcam bubble</div>
                    </div>
                  </div>
                  {!settings.cameraBubble.enabled && (
                    <Check className="w-3.5 h-3.5 text-indigo-400 shrink-0 ml-2" />
                  )}
                </button>

                <div className="h-px bg-studio-800 my-1" />

                {/* Available Cameras */}
                {cameras.length === 0 ? (
                  <div className="px-3 py-3 text-studio-400 text-center text-[11px]">
                    No webcams detected
                  </div>
                ) : (
                  cameras.map((cam) => (
                    <button
                      key={cam.id}
                      type="button"
                      role="menuitemradio"
                      aria-checked={settings.cameraBubble.enabled && settings.selectedCameraId === cam.id}
                      onClick={() => {
                        settings.updateCameraBubble({ enabled: true });
                        settings.setSelectedCameraId(cam.id);
                        setOpenDropdown(null);
                        void onRequestCameraPermission?.();
                      }}
                      className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-left transition-colors ${
                        settings.cameraBubble.enabled && settings.selectedCameraId === cam.id
                          ? "bg-indigo-600/20 text-indigo-200 border border-indigo-500/30 font-medium"
                          : "text-studio-200 hover:bg-studio-800/80"
                      }`}
                    >
                      <div className="flex items-center space-x-2.5 truncate">
                        <Camera className="w-4 h-4 text-indigo-400 shrink-0" />
                        <span className="truncate font-medium">{cam.name}</span>
                      </div>
                      {settings.cameraBubble.enabled && settings.selectedCameraId === cam.id && (
                        <Check className="w-3.5 h-3.5 text-indigo-400 shrink-0 ml-2" />
                      )}
                    </button>
                  ))
                )}
              </div>
            </div>
          )}
          {cameraPermissionProblem && <button id="camera-permission-status" type="button" onClick={() => void onRequestCameraPermission?.()} className="source-permission-issue" aria-label={permissionHint("camera")}>{permissionHint("camera")}</button>}
        </div>

        {/* 4. SYSTEM AUDIO LOOPBACK TOGGLE */}
        <button
          type="button"
          disabled={disabled}
          aria-label="Capture system audio"
          aria-pressed={settings.captureSystemAudio}
          onClick={() => settings.setCaptureSystemAudio(!settings.captureSystemAudio)}
          className={`device-control-card flex w-full min-w-0 items-center space-x-2 px-3 py-2 rounded-xl border transition-all ${
            settings.captureSystemAudio
              ? "bg-emerald-600/20 border-emerald-500/50 text-emerald-200 shadow-sm shadow-emerald-950"
              : "bg-studio-850/80 hover:bg-studio-800 border-studio-750 text-studio-400"
          }`}
          title="Capture system audio (apps, music, calls)"
        >
          {settings.captureSystemAudio ? (
            <Volume2 className="w-4 h-4 text-emerald-400" />
          ) : (
            <VolumeX className="w-4 h-4 text-studio-500" />
          )}
          <div className="flex flex-col text-left">
            <span className="text-[10px] uppercase font-semibold tracking-wider text-studio-400">
              Sys Audio
            </span>
            <span className="font-medium text-white">
              {settings.captureSystemAudio ? "Loopback Active" : "Muted"}
            </span>
          </div>
          {settings.captureSystemAudio && (
            <span className="device-control-detail h-1.5 min-w-10 flex-1 overflow-hidden rounded-full bg-studio-800" title={systemPeakDb === null ? "Waiting for system audio" : `${systemPeakDb.toFixed(1)} dBFS`}>
              <span className={`block h-full rounded-full ${systemPeakDb !== null && systemPeakDb > -3 ? "bg-rose-400" : "bg-emerald-400"}`}
                style={{ width: `${systemPeakDb === null ? 0 : Math.max(0, Math.min(100, ((systemPeakDb + 60) / 60) * 100))}%` }} />
            </span>
          )}
        </button>
      </div>

    </div>
  );
};
