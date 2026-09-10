import React, { useRef, useEffect, useState } from "react";
import { Camera, RefreshCw } from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { CameraBubbleShape, CameraBubbleSize } from "../../lib/types";

export const CameraOverlay: React.FC<{ cameraName?: string }> = ({ cameraName }) => {
  const { cameraBubble, updateCameraBubble, selectedCameraId } = useSettingsStore();
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [hasLiveStream, setHasLiveStream] = useState(false);

  // Attempt to mount live webcam feed if in browser and permitted
  useEffect(() => {
    let activeStream: MediaStream | null = null;
    let mounted = true;

    if (cameraBubble.enabled && typeof navigator !== "undefined" && navigator.mediaDevices?.getUserMedia) {
      const constraints: MediaStreamConstraints = {
        video: selectedCameraId ? { deviceId: { ideal: selectedCameraId } } : true,
        audio: false,
      };

      navigator.mediaDevices
        .getUserMedia(constraints)
        .then((stream) => {
          if (!mounted) {
            stream.getTracks().forEach((t) => t.stop());
            return;
          }
          activeStream = stream;
          if (videoRef.current) {
            videoRef.current.srcObject = stream;
            void videoRef.current.play().catch(() => {});
          }
          setHasLiveStream(true);
        })
        .catch(() => {
          // Camera permission denied or not available; fallback to synthetic avatar
          if (mounted) setHasLiveStream(false);
        });
    }

    return () => {
      mounted = false;
      if (activeStream) {
        activeStream.getTracks().forEach((t) => t.stop());
      }
    };
  }, [cameraBubble.enabled, selectedCameraId]);

  if (!cameraBubble.enabled) return null;

  // Proportional studio size mapping
  const sizeClasses: Record<CameraBubbleSize, string> = {
    sm: "w-24 h-24",
    md: "w-32 h-32",
    lg: "w-40 h-40",
    xl: "w-48 h-48",
  };

  const rectSizeClasses: Record<CameraBubbleSize, string> = {
    sm: "w-36 aspect-video",
    md: "w-48 aspect-video",
    lg: "w-60 aspect-video",
    xl: "w-72 aspect-video",
  };

  // Shape class mapping
  const getShapeStyle = (shape: CameraBubbleShape) => {
    switch (shape) {
      case "rect":
        return "rounded-md";
      case "circle":
        return "rounded-full";
      case "squircle":
        return "rounded-[24px]";
      case "rect_16_9":
        return "rounded-xl";
      default:
        return "rounded-md";
    }
  };

  const currentSizeClass =
    cameraBubble.shape === "rect_16_9"
      ? rectSizeClasses[cameraBubble.size]
      : sizeClasses[cameraBubble.size];

  return (
    <div
      className={`relative overflow-hidden transition-all duration-300 select-none group ${currentSizeClass} ${getShapeStyle(
        cameraBubble.shape
      )} ${cameraBubble.shadow ? "shadow-2xl shadow-black/90" : ""}`}
      style={{
        borderWidth: `${cameraBubble.borderWidth}px`,
        borderColor: cameraBubble.borderColor,
        aspectRatio: cameraBubble.shape === "rect_16_9" ? "16 / 9" : "1 / 1",
      }}
    >
      {/* 1. Camera Feed Layer (Mirrored if set, but ONLY this video/background layer) */}
      <div
        className={`w-full h-full absolute inset-0 bg-gradient-to-br from-indigo-950 via-slate-900 to-zinc-950 flex items-center justify-center transition-transform ${
          cameraBubble.mirror ? "scale-x-[-1]" : ""
        }`}
      >
        <video
          ref={videoRef}
          autoPlay
          playsInline
          muted
          className={`w-full h-full object-cover ${hasLiveStream ? "block" : "hidden"}`}
        />

        {!hasLiveStream && (
          <div className="w-full h-full flex flex-col items-center justify-center relative">
            <Camera className="w-8 h-8 text-indigo-400/70 mb-1" />
            <div className="absolute inset-0 bg-radial-gradient from-transparent to-black/40 pointer-events-none" />
          </div>
        )}
      </div>

      {/* 2. Unmirrored Foreground UI Layer: Always upright text badge & live indicator */}
      <div className="absolute inset-0 flex flex-col items-center justify-end p-2 pointer-events-none z-10">
        <div className="flex items-center space-x-1.5 px-2 py-0.5 rounded-full bg-black/60 backdrop-blur-md border border-white/10 max-w-[90%] shadow">
          <span
            className={`w-1.5 h-1.5 rounded-full ${
              hasLiveStream ? "bg-emerald-400 animate-pulse" : "bg-indigo-400"
            }`}
          />
          <span className="text-[10px] font-medium tracking-wide text-white/90 truncate">
            {cameraName || "Webcam Active"}
          </span>
        </div>
      </div>

      {/* 3. Hover Controls Overlay */}
      <div className="absolute inset-0 bg-black/60 opacity-0 group-hover:opacity-100 flex items-center justify-center gap-2 transition-opacity z-20">
        <button
          type="button"
          onClick={() =>
            updateCameraBubble({
              shape:
                cameraBubble.shape === "circle"
                  ? "squircle"
                  : cameraBubble.shape === "squircle"
                  ? "rect_16_9"
                  : "circle",
            })
          }
          className="p-1.5 rounded-full bg-studio-800/90 hover:bg-studio-700 text-white text-[11px] font-medium border border-studio-600 shadow"
          title="Cycle Shape (Circle / Squircle / 16:9)"
        >
          Shape
        </button>

        <button
          type="button"
          onClick={() => updateCameraBubble({ mirror: !cameraBubble.mirror })}
          className="p-1.5 rounded-full bg-studio-800/90 hover:bg-studio-700 text-white border border-studio-600 shadow"
          title={cameraBubble.mirror ? "Mirror is ON (click to unmirror)" : "Mirror is OFF (click to mirror)"}
        >
          <RefreshCw className="w-3.5 h-3.5" />
        </button>
      </div>
    </div>
  );
};
