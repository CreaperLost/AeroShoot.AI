import React from "react";
import {
  Palette,
  Sliders,
  Camera,
  Maximize2,
  Square,
  Circle,
} from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { CameraBubblePosition, CameraBubbleShape, CameraBubbleSize } from "../../lib/types";

export const InspectorPanel: React.FC = () => {
  const { canvas, cameraBubble, updateCanvas, updateCameraBubble } = useSettingsStore();

  const gradientPresets = [
    { label: "Indigo Cosmic", start: "#312e81", end: "#0f172a" },
    { label: "Electric Violet", start: "#4c1d95", end: "#1e1b4b" },
    { label: "Deep Obsidian", start: "#27272a", end: "#09090b" },
    { label: "Emerald Matrix", start: "#064e3b", end: "#022c22" },
    { label: "Sunset Velvet", start: "#881337", end: "#1e1b4b" },
    { label: "Cyber Ocean", start: "#0c4a6e", end: "#0f172a" },
  ];

  return (
    <div className="w-80 h-full border-l border-studio-800 bg-studio-900/95 flex flex-col overflow-y-auto select-none p-5 space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between pb-3 border-b border-studio-800">
        <div className="flex items-center space-x-2 text-white font-semibold text-sm">
          <Sliders className="w-4 h-4 text-indigo-400" />
          <span>Studio Inspector</span>
        </div>
        <span className="text-[11px] px-2 py-0.5 rounded bg-studio-800 text-studio-400 font-mono">
          Customizer
        </span>
      </div>

      {/* Canvas Wallpaper & Aspect Ratio */}
      <div className="space-y-4">
        <div className="flex items-center space-x-2 text-xs font-semibold uppercase tracking-wider text-studio-400">
          <Palette className="w-3.5 h-3.5 text-indigo-400" />
          <span>Canvas Wallpaper</span>
        </div>

        {/* Gradient Presets */}
        <div className="grid grid-cols-2 gap-2">
          {gradientPresets.map((preset) => (
            <button
              key={preset.label}
              onClick={() =>
                updateCanvas({
                  colorStart: preset.start,
                  colorEnd: preset.end,
                })
              }
              className={`h-12 rounded-lg border text-left p-2 flex flex-col justify-end transition-all ${
                canvas.colorStart === preset.start && canvas.colorEnd === preset.end
                  ? "border-indigo-500 shadow-md shadow-indigo-500/20"
                  : "border-studio-750 hover:border-studio-600"
              }`}
              style={{
                background: `linear-gradient(135deg, ${preset.start}, ${preset.end})`,
              }}
            >
              <span className="text-[10px] font-medium text-white/90 drop-shadow">
                {preset.label}
              </span>
            </button>
          ))}
        </div>

        {/* Aspect Ratio Picker */}
        <div className="space-y-1.5 pt-1">
          <label className="text-xs text-studio-400">Aspect Ratio</label>
          <div className="grid grid-cols-4 gap-1.5 bg-studio-850 p-1 rounded-lg border border-studio-800">
            {(["16:9", "9:16", "4:3", "1:1"] as const).map((ratio) => (
              <button
                key={ratio}
                onClick={() => updateCanvas({ aspectRatio: ratio })}
                className={`py-1 text-xs font-mono rounded transition-colors ${
                  canvas.aspectRatio === ratio
                    ? "bg-indigo-600 text-white font-semibold"
                    : "text-studio-400 hover:text-studio-200"
                }`}
              >
                {ratio}
              </button>
            ))}
          </div>
        </div>

        {/* Padding Slider */}
        <div className="space-y-1.5">
          <div className="flex justify-between text-xs">
            <span className="text-studio-400">Canvas Padding</span>
            <span className="font-mono text-studio-300">{canvas.paddingPx}px</span>
          </div>
          <input
            type="range"
            min={0}
            max={80}
            value={canvas.paddingPx}
            onChange={(e) => updateCanvas({ paddingPx: Number(e.target.value) })}
            className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
          />
        </div>

        {/* Corner Radius Slider */}
        <div className="space-y-1.5">
          <div className="flex justify-between text-xs">
            <span className="text-studio-400">Corner Radius</span>
            <span className="font-mono text-studio-300">{canvas.cornerRadiusPx}px</span>
          </div>
          <input
            type="range"
            min={0}
            max={32}
            value={canvas.cornerRadiusPx}
            onChange={(e) => updateCanvas({ cornerRadiusPx: Number(e.target.value) })}
            className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
          />
        </div>

        {/* Drop Shadow Blur */}
        <div className="space-y-1.5">
          <div className="flex justify-between text-xs">
            <span className="text-studio-400">Drop Shadow</span>
            <span className="font-mono text-studio-300">{canvas.shadowBlurPx}px</span>
          </div>
          <input
            type="range"
            min={0}
            max={40}
            value={canvas.shadowBlurPx}
            onChange={(e) => updateCanvas({ shadowBlurPx: Number(e.target.value) })}
            className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
          />
        </div>
      </div>

      <div className="h-px bg-studio-800" />

      {/* Webcam Bubble Settings */}
      <div className="space-y-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center space-x-2 text-xs font-semibold uppercase tracking-wider text-studio-400">
            <Camera className="w-3.5 h-3.5 text-indigo-400" />
            <span>Webcam Bubble</span>
          </div>
          <input
            type="checkbox"
            checked={cameraBubble.enabled}
            onChange={(e) => updateCameraBubble({ enabled: e.target.checked })}
            className="rounded bg-studio-800 border-studio-700 text-indigo-600 focus:ring-0 cursor-pointer"
          />
        </div>

        {cameraBubble.enabled && (
          <>
            {/* Shape Select */}
            <div className="space-y-1.5">
              <label className="text-xs text-studio-400">Shape</label>
              <div className="grid grid-cols-3 gap-1.5 bg-studio-850 p-1 rounded-lg border border-studio-800">
                {(
                  [
                    { key: "circle", label: "Circle", icon: Circle },
                    { key: "squircle", label: "Squircle", icon: Square },
                    { key: "rect_16_9", label: "16:9", icon: Maximize2 },
                  ] as const
                ).map(({ key, label }) => (
                  <button
                    key={key}
                    onClick={() => updateCameraBubble({ shape: key as CameraBubbleShape })}
                    className={`py-1 text-xs rounded transition-colors ${
                      cameraBubble.shape === key
                        ? "bg-indigo-600 text-white font-medium"
                        : "text-studio-400 hover:text-studio-200"
                    }`}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>

            {/* Bubble Size */}
            <div className="space-y-1.5">
              <label className="text-xs text-studio-400">Size</label>
              <div className="grid grid-cols-4 gap-1.5 bg-studio-850 p-1 rounded-lg border border-studio-800">
                {(["sm", "md", "lg", "xl"] as const).map((size) => (
                  <button
                    key={size}
                    onClick={() => updateCameraBubble({ size: size as CameraBubbleSize })}
                    className={`py-1 text-xs uppercase font-mono rounded transition-colors ${
                      cameraBubble.size === size
                        ? "bg-indigo-600 text-white font-medium"
                        : "text-studio-400 hover:text-studio-200"
                    }`}
                  >
                    {size}
                  </button>
                ))}
              </div>
            </div>

            {/* Position Presets */}
            <div className="space-y-1.5">
              <label className="text-xs text-studio-400">Position</label>
              <div className="grid grid-cols-2 gap-1.5">
                {(
                  [
                    { key: "bottom-right", label: "Bottom Right" },
                    { key: "bottom-left", label: "Bottom Left" },
                    { key: "top-right", label: "Top Right" },
                    { key: "top-left", label: "Top Left" },
                  ] as const
                ).map(({ key, label }) => (
                  <button
                    key={key}
                    onClick={() => updateCameraBubble({ position: key as CameraBubblePosition })}
                    className={`py-1.5 px-2 text-xs rounded-md border text-left transition-colors ${
                      cameraBubble.position === key
                        ? "bg-indigo-600/20 border-indigo-500/40 text-indigo-200 font-medium"
                        : "bg-studio-850 border-studio-800 text-studio-400 hover:text-studio-200"
                    }`}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>

            {/* Border Width & Color */}
            <div className="space-y-1.5">
              <div className="flex justify-between text-xs">
                <span className="text-studio-400">Border Width</span>
                <span className="font-mono text-studio-300">{cameraBubble.borderWidth}px</span>
              </div>
              <input
                type="range"
                min={0}
                max={8}
                value={cameraBubble.borderWidth}
                onChange={(e) => updateCameraBubble({ borderWidth: Number(e.target.value) })}
                className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
              />
            </div>
          </>
        )}
      </div>
    </div>
  );
};
