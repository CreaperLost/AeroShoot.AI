import React from "react";
import { MousePointer, Sparkles, Smartphone, Monitor } from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { useProjectStore } from "../../stores/projectStore";
import { CameraOverlay } from "../camera-overlay/CameraOverlay";
import { CameraDevice } from "../../lib/types";

interface StudioCanvasProps {
  activeCamera?: CameraDevice;
}

export const StudioCanvas: React.FC<StudioCanvasProps> = ({ activeCamera }) => {
  const { canvas, cameraBubble } = useSettingsStore();
  const { zoomKeyframes, currentTimeUs, durationUs } = useProjectStore();

  // Compute active zoom scale based on current playhead time
  const activeKeyframe = zoomKeyframes
    .filter((k) => k.tUs <= currentTimeUs)
    .sort((a, b) => b.tUs - a.tUs)[0];
  const currentZoomScale = activeKeyframe ? activeKeyframe.scale : 1.0;
  const currentNormX = activeKeyframe ? activeKeyframe.normX : 0.5;
  const currentNormY = activeKeyframe ? activeKeyframe.normY : 0.5;

  // Aspect ratio sizing classes calibrated to leave breathing room on all displays
  const getContainerSizeClasses = () => {
    switch (canvas.aspectRatio) {
      case "16:9":
        return "w-full max-w-4xl max-h-[440px] aspect-[16/9]";
      case "9:16":
        return "h-full max-h-[460px] max-w-[270px] aspect-[9/16]";
      case "4:3":
        return "w-full max-w-3xl max-h-[440px] aspect-[4/3]";
      case "1:1":
        return "h-full max-h-[420px] max-w-[420px] aspect-square";
      default:
        return "w-full max-w-4xl max-h-[440px] aspect-[16/9]";
    }
  };

  // Camera bubble position styling inside studio preview
  const getBubblePositionStyle = () => {
    switch (cameraBubble.position) {
      case "bottom-right":
        return "bottom-3 right-3";
      case "bottom-left":
        return "bottom-3 left-3";
      case "top-right":
        return "top-3 right-3";
      case "top-left":
        return "top-3 left-3";
      default:
        return "bottom-3 right-3";
    }
  };

  const isVertical = canvas.aspectRatio === "9:16";

  return (
    <div className="w-full h-full flex flex-col items-center justify-center relative overflow-hidden select-none">
      {/* Dynamic Aspect Ratio Canvas Frame with glowing drop shadow */}
      <div
        className={`rounded-2xl flex items-center justify-center relative transition-all duration-300 ease-out overflow-hidden shadow-2xl border border-white/10 ${getContainerSizeClasses()}`}
        style={{
          background:
            canvas.backgroundType === "gradient"
              ? `linear-gradient(135deg, ${canvas.colorStart}, ${canvas.colorEnd})`
              : canvas.colorStart,
          padding: `${canvas.paddingPx}px`,
          aspectRatio:
            canvas.aspectRatio === "16:9"
              ? "16/9"
              : canvas.aspectRatio === "9:16"
              ? "9/16"
              : canvas.aspectRatio === "4:3"
              ? "4/3"
              : "1/1",
        }}
      >
        {/* Aspect Ratio Badge */}
        <div className="absolute top-3 right-3 z-30 flex items-center space-x-1.5 px-2.5 py-1 rounded-full bg-black/60 backdrop-blur-md text-[10px] font-mono text-studio-300 border border-white/10 shadow">
          {isVertical ? (
            <Smartphone className="w-3 h-3 text-indigo-400" />
          ) : (
            <Monitor className="w-3 h-3 text-emerald-400" />
          )}
          <span>{canvas.aspectRatio}</span>
          <span className="text-studio-500">
            {canvas.aspectRatio === "16:9"
              ? "Widescreen"
              : canvas.aspectRatio === "9:16"
              ? "Reels / Shorts"
              : canvas.aspectRatio === "4:3"
              ? "Standard"
              : "Square"}
          </span>
        </div>

        {/* Screen Capture Canvas Window Frame */}
        <div
          className={`w-full ${
            isVertical ? "h-[72%] my-auto" : "h-full"
          } relative overflow-hidden bg-slate-900 transition-all duration-300 flex items-center justify-center`}
          style={{
            borderRadius: `${canvas.cornerRadiusPx}px`,
            boxShadow: `0 20px ${canvas.shadowBlurPx}px rgba(0,0,0,${canvas.shadowOpacity})`,
          }}
        >
          {/* Mock Screen Content with Zoom Transform */}
          <div
            className="w-full h-full bg-gradient-to-tr from-slate-950 via-zinc-900 to-indigo-950/40 p-5 flex flex-col justify-between transition-transform duration-500 ease-out origin-center"
            style={{
              transform: `scale(${currentZoomScale}) translate(${
                (0.5 - currentNormX) * 20
              }%, ${(0.5 - currentNormY) * 20}%)`,
            }}
          >
            {/* Mock Application Window Header */}
            <div className="flex items-center space-x-2 border-b border-white/10 pb-2.5">
              <div className="flex space-x-1.5">
                <div className="w-2.5 h-2.5 rounded-full bg-rose-500/80" />
                <div className="w-2.5 h-2.5 rounded-full bg-amber-500/80" />
                <div className="w-2.5 h-2.5 rounded-full bg-emerald-500/80" />
              </div>
              <span className="text-[10px] font-mono text-white/50 pl-2 truncate">
                AeroShoot.AI Studio — Telemetry &amp; Vector Compositor
              </span>
            </div>

            {/* Mock Workspace Content */}
            <div className={`grid ${isVertical ? "grid-cols-1 gap-2" : "grid-cols-3 gap-2.5"} my-auto`}>
              <div className="p-2.5 rounded-xl bg-white/5 border border-white/10 backdrop-blur-sm">
                <div className="text-[11px] text-indigo-400 font-semibold mb-0.5">
                  Multi-Stream Capture
                </div>
                <div className="text-[10px] text-white/70 leading-snug">
                  Screen, Webcam, Loopback Audio, and Mic tracks.
                </div>
              </div>

              <div className="p-2.5 rounded-xl bg-white/5 border border-white/10 backdrop-blur-sm">
                <div className="text-[11px] text-emerald-400 font-semibold mb-0.5">
                  Smart Auto-Zoom
                </div>
                <div className="text-[10px] text-white/70 leading-snug">
                  Telemetry cursor tracking for smooth cinematic pans.
                </div>
              </div>

              {!isVertical && (
                <div className="p-2.5 rounded-xl bg-white/5 border border-white/10 backdrop-blur-sm">
                  <div className="text-[11px] text-rose-400 font-semibold mb-0.5">
                    AI Silence Cuts
                  </div>
                  <div className="text-[10px] text-white/70 leading-snug">
                    DSP dead-air removal across all synced media tracks.
                  </div>
                </div>
              )}
            </div>

            {/* Synthetic Cursor Overlay */}
            <div
              className="absolute pointer-events-none transition-all duration-150 flex items-center space-x-1.5"
              style={{
                left: `${currentNormX * 85}%`,
                top: `${currentNormY * 85}%`,
              }}
            >
              <MousePointer className="w-4 h-4 text-white drop-shadow-[0_2px_8px_rgba(0,0,0,0.8)] fill-white" />
              {currentZoomScale > 1.0 && (
                <span className="px-1.5 py-0.5 rounded bg-indigo-600/90 text-[9px] font-mono text-white shadow">
                  {currentZoomScale}x
                </span>
              )}
            </div>

            {/* Footer bar */}
            <div className="text-[10px] font-mono text-white/40 flex justify-between pt-1 border-t border-white/5">
              <span>60fps Compositor</span>
              <span>
                {(currentTimeUs / 1_000_000).toFixed(1)}s / {(durationUs / 1_000_000).toFixed(1)}s
              </span>
            </div>
          </div>

          {/* Floating Camera Bubble inside Screen/Canvas */}
          {cameraBubble.enabled && (
            <div className={`absolute ${getBubblePositionStyle()} z-20`}>
              <CameraOverlay cameraName={activeCamera?.name} />
            </div>
          )}
        </div>

        {/* Quick Zoom Indicator badge */}
        {currentZoomScale > 1.0 && (
          <div className="absolute top-3 left-3 z-30 flex items-center space-x-1.5 px-3 py-1 rounded-full bg-indigo-600/80 backdrop-blur-md text-white text-[11px] font-semibold shadow-lg">
            <Sparkles className="w-3.5 h-3.5 text-amber-300" />
            <span>Smart Zoom ({currentZoomScale}x)</span>
          </div>
        )}
      </div>
    </div>
  );
};
