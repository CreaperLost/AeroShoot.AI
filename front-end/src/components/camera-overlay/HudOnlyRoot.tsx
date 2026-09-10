import React, { useEffect } from "react";
import { Camera, Circle, Maximize2, RefreshCw, Square } from "lucide-react";
import { NativePreviewHost } from "../canvas/NativePreviewHost";
import { useHudSettings } from "../../hooks/useHudSettings";
import { useSettingsStore } from "../../stores/settingsStore";
import { api } from "../../lib/ipc";
import { CameraBubbleShape, CameraBubbleSize } from "../../lib/types";

const SHAPES: CameraBubbleShape[] = ["circle", "squircle", "rect"];
const SIZES: CameraBubbleSize[] = ["sm", "md", "lg", "xl"];

/**
 * HUD-only window root. Camera chrome and settings controls only.
 * Does not mount Record/Edit studio or start a recording session.
 */
export const HudOnlyRoot: React.FC = () => {
  useHudSettings();
  const cameraBubble = useSettingsStore((s) => s.cameraBubble);
  const hudCameraAvailable = useSettingsStore((s) => s.hudCameraAvailable);
  const hudDiagnostics = useSettingsStore((s) => s.hudDiagnostics);
  const selectedCameraId = useSettingsStore((s) => s.selectedCameraId);
  const updateCameraBubble = useSettingsStore((s) => s.updateCameraBubble);

  useEffect(() => {
    document.documentElement.classList.add("hud-root");
    document.body.classList.add("hud-root");
    return () => {
      document.documentElement.classList.remove("hud-root");
      document.body.classList.remove("hud-root");
    };
  }, []);

  const cycleShape = () => {
    const index = SHAPES.indexOf(cameraBubble.shape);
    updateCameraBubble({ shape: SHAPES[(index + 1) % SHAPES.length] });
  };

  const cycleSize = () => {
    const index = SIZES.indexOf(cameraBubble.size);
    updateCameraBubble({ size: SIZES[(index + 1) % SIZES.length] });
  };

  const hitMode =
    cameraBubble.shape === "circle"
      ? "circle_pass_through"
      : cameraBubble.shape === "squircle"
        ? "squircle_pass_through"
        : "pass_through";
  const size = sizeClass(cameraBubble.shape, cameraBubble.size);

  return (
    <div
      data-ui-root="hud"
      data-hud-only
      className={`relative h-screen w-screen overflow-hidden select-none ${size} ${shapeClass(cameraBubble.shape)}`}
      style={{
        borderWidth: cameraBubble.borderWidth,
        borderColor: cameraBubble.borderColor,
        borderStyle: "solid",
      }}
    >
      <div data-tauri-drag-region className="absolute inset-0 z-0" />
      <div
        className={`absolute inset-0 pointer-events-none ${cameraBubble.mirror ? "scale-x-[-1]" : ""}`}
      >
        <NativePreviewHost
          live
          surface="hud"
          windowLabel="camera_overlay"
          hitMode={hitMode}
          className="w-full h-full rounded-none border-0"
          showStatus={false}
        />
      </div>
      {!hudCameraAvailable && (
        <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/70 text-white z-10 pointer-events-none">
          <Camera className="w-8 h-8 mb-2 text-amber-300" />
          <p className="text-xs font-medium">Camera removed</p>
          {selectedCameraId && (
            <p className="text-[10px] text-white/70 mt-1 px-4 text-center truncate max-w-full">
              {selectedCameraId}
            </p>
          )}
        </div>
      )}
      <div className="absolute bottom-2 inset-x-0 flex justify-center gap-1.5 z-20">
        <button
          type="button"
          onClick={cycleShape}
          className="p-1.5 rounded-full bg-black/70 text-white border border-white/20"
          title="Cycle shape"
        >
          {cameraBubble.shape === "circle" ? (
            <Circle className="w-3.5 h-3.5" />
          ) : cameraBubble.shape === "rect_16_9" ? (
            <Maximize2 className="w-3.5 h-3.5" />
          ) : (
            <Square className="w-3.5 h-3.5" />
          )}
        </button>
        <button
          type="button"
          onClick={cycleSize}
          className="px-2 py-1 rounded-full bg-black/70 text-white text-[10px] font-mono uppercase border border-white/20"
          title="Cycle size"
        >
          {cameraBubble.size}
        </button>
        <button
          type="button"
          onClick={() => updateCameraBubble({ mirror: !cameraBubble.mirror })}
          className="p-1.5 rounded-full bg-black/70 text-white border border-white/20"
          title={cameraBubble.mirror ? "Unmirror" : "Mirror"}
        >
          <RefreshCw className="w-3.5 h-3.5" />
        </button>
        <button
          type="button"
          onClick={() => void api.hudSetVisible(false)}
          className="px-2 py-1 rounded-full bg-black/70 text-white text-[10px] border border-white/20"
          title="Hide HUD (does not stop recording)"
        >
          Close
        </button>
      </div>
      {hudDiagnostics[0] && (
        <p className="absolute top-1 inset-x-2 text-[9px] text-amber-200/90 text-center pointer-events-none">
          {hudDiagnostics[0]}
        </p>
      )}
    </div>
  );
};

function shapeClass(shape: CameraBubbleShape): string {
  switch (shape) {
    case "circle":
      return "rounded-full";
    case "squircle":
      return "rounded-[28px]";
    case "rect_16_9":
      return "rounded-xl";
    default:
      return "rounded-md";
  }
}

function sizeClass(shape: CameraBubbleShape, size: CameraBubbleSize): string {
  if (shape === "rect_16_9") {
    return {
      sm: "min-w-[200px] aspect-video",
      md: "min-w-[280px] aspect-video",
      lg: "min-w-[360px] aspect-video",
      xl: "min-w-[440px] aspect-video",
    }[size];
  }
  return {
    sm: "min-w-[160px] min-h-[160px]",
    md: "min-w-[240px] min-h-[240px]",
    lg: "min-w-[320px] min-h-[320px]",
    xl: "min-w-[400px] min-h-[400px]",
  }[size];
}

export const RejectedWindowRoot: React.FC = () => (
  <div
    data-ui-root="rejected"
    className="h-screen w-screen flex items-center justify-center bg-studio-950 text-studio-400 text-sm"
  >
    This window is not a valid AeroShoot root.
  </div>
);
