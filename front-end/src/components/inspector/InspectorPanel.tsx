import React from "react";
import {
  Palette,
  Sliders,
  Camera,
} from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import {
  CameraBubblePosition,
  CameraBubbleSize,
  LAYOUT_UNSUPPORTED,
} from "../../lib/types";

export const InspectorPanel: React.FC = () => {
  const {
    canvas,
    cameraBubble,
    updateCanvas,
    updateCameraBubble,
  } = useSettingsStore();

  const setCanvas = (patch: Parameters<typeof updateCanvas>[0]) => {
    updateCanvas(patch);
  };

  const setCamera = (patch: Parameters<typeof updateCameraBubble>[0]) => {
    updateCameraBubble(patch);
  };

  const gradientPresets = [
    { label: "Indigo Cosmic", start: "#312e81", end: "#0f172a" },
    { label: "Electric Violet", start: "#4c1d95", end: "#1e1b4b" },
    { label: "Deep Obsidian", start: "#27272a", end: "#09090b" },
    { label: "Emerald Matrix", start: "#064e3b", end: "#022c22" },
    { label: "Sunset Velvet", start: "#881337", end: "#1e1b4b" },
    { label: "Cyber Ocean", start: "#0c4a6e", end: "#0f172a" },
  ];

  return (
    <div className="studio-inspector min-w-0 h-full border-l border-studio-800 bg-studio-900/95 flex flex-col overflow-y-auto select-none p-5 space-y-6">
      <div className="flex items-center justify-between pb-3 border-b border-studio-800">
        <div className="flex items-center space-x-2 text-white font-semibold text-sm">
          <Sliders className="w-4 h-4 text-indigo-400" />
          <span>Studio Customizer</span>
        </div>
        <span className="text-[11px] px-2 py-0.5 rounded bg-studio-800 text-studio-400 font-mono">
          Settings
        </span>
      </div>

      <div className="space-y-4">
        <div className="flex items-center space-x-2 text-xs font-semibold uppercase tracking-wider text-studio-400">
          <Palette className="w-3.5 h-3.5 text-indigo-400" />
          <span>Canvas Wallpaper</span>
        </div>

        <div className="space-y-1.5">
          <label className="text-xs text-studio-400">Background</label>
          <div className="grid grid-cols-3 gap-1.5 bg-studio-850 p-1 rounded-lg border border-studio-800">
            {(["solid", "gradient", "wallpaper"] as const).map((kind) => (
              <button
                key={kind}
                type="button"
                onClick={() => {
                  setCanvas({ backgroundType: kind });
                }}
                className={`py-1 text-xs capitalize rounded transition-colors ${
                  canvas.backgroundType === kind
                    ? "bg-indigo-600 text-white font-medium"
                    : "text-studio-400 hover:text-studio-200"
                }`}
              >
                {kind === "wallpaper" ? "Image" : kind}
              </button>
            ))}
          </div>
          {canvas.backgroundType === "wallpaper" && (
            <p className="text-[10px] text-studio-500">
              Stored in the project bundle. Export never reads an external URL.
            </p>
          )}
        </div>

        <div className="grid grid-cols-2 gap-2">
          {gradientPresets.map((preset) => (
            <button
              key={preset.label}
              onClick={() =>
                setCanvas({
                  backgroundType: "gradient",
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

        <div className="grid grid-cols-2 gap-2">
          <label className="text-xs text-studio-400 space-y-1">
            <span>Start</span>
            <input
              type="color"
              value={canvas.colorStart}
              onChange={(e) => setCanvas({ colorStart: e.target.value })}
              className="w-full h-8 bg-studio-850 border border-studio-800 rounded cursor-pointer"
            />
          </label>
          <label className="text-xs text-studio-400 space-y-1">
            <span>End</span>
            <input
              type="color"
              value={canvas.colorEnd}
              onChange={(e) => setCanvas({ colorEnd: e.target.value })}
              disabled={canvas.backgroundType === "solid"}
              className="w-full h-8 bg-studio-850 border border-studio-800 rounded cursor-pointer disabled:opacity-40"
            />
          </label>
        </div>

        <div className="space-y-1.5 pt-1">
          <label className="text-xs text-studio-400">Aspect Ratio</label>
          <div className="grid grid-cols-4 gap-1.5 bg-studio-850 p-1 rounded-lg border border-studio-800">
            {(["16:9", "9:16", "4:3", "1:1"] as const).map((ratio) => (
              <button
                key={ratio}
                onClick={() => setCanvas({ aspectRatio: ratio })}
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
            onChange={(e) => setCanvas({ paddingPx: Number(e.target.value) })}
            className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
          />
        </div>

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
            onChange={(e) => setCanvas({ cornerRadiusPx: Number(e.target.value) })}
            className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
          />
        </div>

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
            onChange={(e) => setCanvas({ shadowBlurPx: Number(e.target.value) })}
            className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
          />
        </div>
      </div>

      <div className="h-px bg-studio-800" />

      <div className="space-y-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center space-x-2 text-xs font-semibold uppercase tracking-wider text-studio-400">
            <Camera className="w-3.5 h-3.5 text-indigo-400" />
            <span>Webcam Bubble</span>
          </div>
          <input
            type="checkbox"
            checked={cameraBubble.enabled}
            onChange={(e) => setCamera({ enabled: e.target.checked })}
            className="rounded bg-studio-800 border-studio-700 text-indigo-600 focus:ring-0 cursor-pointer"
          />
        </div>

        {cameraBubble.enabled && (
          <>
            <div className="space-y-1.5">
              <label className="text-xs text-studio-400">Shape</label>
              <div className="grid grid-cols-4 gap-1.5 bg-studio-850 p-1 rounded-lg border border-studio-800">
                {(
                  [
                    { key: "rect", label: "Rect" },
                    { key: "circle", label: "Circle" },
                    { key: "squircle", label: "Squircle" },
                  ] as const
                ).map(({ key, label }) => (
                  <button
                    key={key}
                    type="button"
                    onClick={() => setCamera({ shape: key })}
                    className={`py-1 text-xs rounded transition-colors ${
                      cameraBubble.shape === key
                        ? "bg-indigo-600 text-white font-medium"
                        : "text-studio-400 hover:text-studio-200"
                    }`}
                  >
                    {label}
                  </button>
                ))}
                <button
                  type="button"
                  disabled
                  title={LAYOUT_UNSUPPORTED.rect169}
                  className="py-1 text-xs rounded text-studio-600 cursor-not-allowed"
                >
                  16:9
                </button>
              </div>
              {cameraBubble.shape === "rect_16_9" && (
                <p className="text-[10px] text-studio-500">{LAYOUT_UNSUPPORTED.rect169}</p>
              )}
            </div>

            <div className="space-y-1.5">
              <label className="text-xs text-studio-400">Size</label>
              <div className="grid grid-cols-4 gap-1.5 bg-studio-850 p-1 rounded-lg border border-studio-800">
                {(["sm", "md", "lg", "xl"] as const).map((size) => (
                  <button
                    key={size}
                    onClick={() => setCamera({ size: size as CameraBubbleSize })}
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

            <div className="space-y-1.5">
              <label className="text-xs text-studio-400">Position</label>
              <div className="grid grid-cols-2 gap-1.5">
                {(
                  [
                    { key: "bottom-right", label: "Bottom Right" },
                    { key: "bottom-left", label: "Bottom Left" },
                    { key: "top-right", label: "Top Right" },
                    { key: "top-left", label: "Top Left" },
                    { key: "custom", label: "Custom" },
                  ] as const
                ).map(({ key, label }) => (
                  <button
                    key={key}
                    onClick={() => setCamera({ position: key as CameraBubblePosition })}
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

            {cameraBubble.position === "custom" && (
              <div className="space-y-2">
                <div className="flex justify-between text-xs">
                  <span className="text-studio-400">Custom X</span>
                  <span className="font-mono text-studio-300">{Math.round(cameraBubble.customX)}%</span>
                </div>
                <input
                  type="range"
                  min={0}
                  max={100}
                  value={cameraBubble.customX}
                  onChange={(e) => setCamera({ customX: Number(e.target.value) })}
                  className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
                />
                <div className="flex justify-between text-xs">
                  <span className="text-studio-400">Custom Y</span>
                  <span className="font-mono text-studio-300">{Math.round(cameraBubble.customY)}%</span>
                </div>
                <input
                  type="range"
                  min={0}
                  max={100}
                  value={cameraBubble.customY}
                  onChange={(e) => setCamera({ customY: Number(e.target.value) })}
                  className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
                />
              </div>
            )}

            <label className="flex items-center justify-between text-xs text-studio-400">
              <span>Mirror webcam</span>
              <input
                type="checkbox"
                checked={cameraBubble.mirror}
                onChange={(e) => setCamera({ mirror: e.target.checked })}
                className="rounded bg-studio-800 border-studio-700 text-indigo-600 focus:ring-0 cursor-pointer"
              />
            </label>

            <label className="flex items-center justify-between text-xs text-studio-400">
              <span>Webcam shadow</span>
              <input
                type="checkbox"
                checked={cameraBubble.shadow}
                onChange={(e) => setCamera({ shadow: e.target.checked })}
                className="rounded bg-studio-800 border-studio-700 text-indigo-600 focus:ring-0 cursor-pointer"
              />
            </label>

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
                onChange={(e) => setCamera({ borderWidth: Number(e.target.value) })}
                className="w-full accent-indigo-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
              />
            </div>
          </>
        )}
      </div>
    </div>
  );
};
