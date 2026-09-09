import React, { useState, useRef, useEffect } from "react";
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
import { CaptureSource, CameraDevice, AudioDevice } from "../../lib/types";

interface DeviceControlDeckProps {
  sources: CaptureSource[];
  cameras: CameraDevice[];
  mics: AudioDevice[];
  disabled?: boolean;
}

export const DeviceControlDeck: React.FC<DeviceControlDeckProps> = ({
  sources,
  cameras,
  mics,
  disabled = false,
}) => {
  const settings = useSettingsStore();

  const [openDropdown, setOpenDropdown] = useState<"source" | "mic" | "camera" | null>(null);

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

  const displays = sources.filter((s) => s.sourceType === "display");
  const selectedSource = displays.find((s) => s.id === settings.selectedSourceId) ?? displays[0] ?? sources[0];
  const selectedMic = mics.find((m) => m.id === settings.selectedMicId) ?? mics[0];
  const selectedCamera = cameras.find((c) => c.id === settings.selectedCameraId) ?? cameras[0];

  return (
    <div
      ref={containerRef}
      className="w-full bg-studio-900/80 border-b border-studio-800/80 px-6 py-2.5 flex flex-wrap items-center justify-between gap-3 text-xs z-20 select-none backdrop-blur-md"
    >
      <div className="flex flex-wrap items-center gap-2.5">
        {/* 1. PHYSICAL DISPLAY SELECTOR */}
        <div className="relative">
          <button
            type="button"
            disabled={disabled}
            onClick={() => setOpenDropdown(openDropdown === "source" ? null : "source")}
            className={`flex items-center space-x-2.5 px-3 py-2 rounded-xl border transition-all ${
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
              <span className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-studio-800 border border-studio-700 text-studio-400">
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
            <div className="absolute top-full mt-2 left-0 w-80 bg-studio-900 border border-studio-700/80 rounded-2xl shadow-2xl p-2 z-50 animate-in fade-in zoom-in-95 backdrop-blur-xl">
              <div className="text-[11px] font-semibold text-studio-400 uppercase px-3 py-1.5 border-b border-studio-800 flex justify-between">
                <span>Displays</span>
                <span className="text-[10px] text-studio-500 font-mono">
                  {displays.length} detected
                </span>
              </div>
              <div className="py-1 space-y-0.5 max-h-56 overflow-y-auto">
                {displays.length === 0 ? (
                  <div className="px-3 py-3 text-studio-400 text-center">No displays detected</div>
                ) : (
                  displays.map((disp) => (
                    <button
                      key={disp.id}
                      type="button"
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
        </div>

        {/* 2. MICROPHONE / AUDIO SELECTOR */}
        <div className="relative">
          <button
            type="button"
            disabled={disabled}
            onClick={() => setOpenDropdown(openDropdown === "mic" ? null : "mic")}
            className={`flex items-center space-x-2.5 px-3 py-2 rounded-xl border transition-all ${
              openDropdown === "mic"
                ? "bg-studio-800 border-amber-500/80 text-white shadow-lg shadow-amber-500/10"
                : "bg-studio-850/80 hover:bg-studio-800 border-studio-750 text-studio-200"
            }`}
          >
            <div className="p-1 rounded-lg bg-amber-500/15 text-amber-400 relative">
              <Mic className="w-4 h-4" />
              {/* Animated VU Meter Dot */}
              {selectedMic && (
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
            {/* Live audio level visualizer bar */}
            {selectedMic && (
              <div className="flex items-end space-x-0.5 h-3.5 px-1 py-0.5 bg-studio-800/80 rounded border border-studio-700/60">
                <div className="w-0.5 h-1.5 bg-emerald-400 rounded-full animate-bounce" style={{ animationDuration: "600ms" }} />
                <div className="w-0.5 h-3 bg-emerald-400 rounded-full animate-bounce" style={{ animationDuration: "450ms" }} />
                <div className="w-0.5 h-2 bg-emerald-400 rounded-full animate-bounce" style={{ animationDuration: "750ms" }} />
                <div className="w-0.5 h-2.5 bg-amber-400 rounded-full animate-bounce" style={{ animationDuration: "550ms" }} />
              </div>
            )}
            <ChevronDown
              className={`w-3.5 h-3.5 text-studio-400 transition-transform ${
                openDropdown === "mic" ? "rotate-180" : ""
              }`}
            />
          </button>

          {openDropdown === "mic" && (
            <div className="absolute top-full mt-2 left-0 w-72 bg-studio-900 border border-studio-700/80 rounded-2xl shadow-2xl p-2 z-50 animate-in fade-in zoom-in-95 backdrop-blur-xl">
              <div className="text-[11px] font-semibold text-studio-400 uppercase px-3 py-1.5 border-b border-studio-800 flex justify-between">
                <span>Audio Inputs</span>
                <span className="text-[10px] text-studio-500 font-mono">{mics.length} detected</span>
              </div>
              <div className="py-1 space-y-0.5 max-h-56 overflow-y-auto">
                {mics.length === 0 ? (
                  <div className="px-3 py-3 text-studio-400 text-center">No microphones detected</div>
                ) : (
                  mics.map((mic) => (
                    <button
                      key={mic.id}
                      type="button"
                      onClick={() => {
                        settings.setSelectedMicId(mic.id);
                        setOpenDropdown(null);
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
            </div>
          )}
        </div>

        {/* 3. WEBCAMERA SELECTOR (Unified card with 'No Camera' option) */}
        <div className="relative">
          <button
            type="button"
            disabled={disabled}
            onClick={() => setOpenDropdown(openDropdown === "camera" ? null : "camera")}
            className={`flex items-center space-x-2.5 px-3 py-2 rounded-xl border transition-all ${
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
            <div className="absolute top-full mt-2 left-0 w-72 bg-studio-900 border border-studio-700/80 rounded-2xl shadow-2xl p-2 z-50 animate-in fade-in zoom-in-95 backdrop-blur-xl">
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
                      onClick={() => {
                        settings.updateCameraBubble({ enabled: true });
                        settings.setSelectedCameraId(cam.id);
                        setOpenDropdown(null);
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
        </div>

        {/* 4. SYSTEM AUDIO LOOPBACK TOGGLE */}
        <button
          type="button"
          disabled={disabled}
          onClick={() => settings.setCaptureSystemAudio(!settings.captureSystemAudio)}
          className={`flex items-center space-x-2 px-3 py-2 rounded-xl border transition-all ${
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
        </button>
      </div>

      {/* 5. ASPECT RATIO SELECTOR (Direct, immediate effect!) */}
      <div className="flex items-center space-x-2 bg-studio-950/70 p-1 rounded-xl border border-studio-800">
        <span className="text-[11px] font-semibold text-studio-400 uppercase px-2 font-mono">
          Ratio
        </span>
        {(
          [
            { key: "16:9", label: "16:9", hint: "Landscape" },
            { key: "9:16", label: "9:16", hint: "Vertical / Mobile" },
            { key: "4:3", label: "4:3", hint: "Standard" },
            { key: "1:1", label: "1:1", hint: "Square" },
          ] as const
        ).map((r) => (
          <button
            key={r.key}
            type="button"
            onClick={() => settings.updateCanvas({ aspectRatio: r.key })}
            title={r.hint}
            className={`px-2.5 py-1 rounded-lg text-xs font-mono font-medium transition-all ${
              settings.canvas.aspectRatio === r.key
                ? "bg-indigo-600 text-white shadow-md shadow-indigo-600/30 scale-105"
                : "text-studio-400 hover:text-white hover:bg-studio-850"
            }`}
          >
            {r.label}
          </button>
        ))}
      </div>
    </div>
  );
};
