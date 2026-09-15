import React from "react";
import { RotateCcw } from "lucide-react";

const MIC_GAIN_RANGE = { min: -24, max: 24 } as const;

/**
 * Slider that controls the live mic gain (in decibels). 0 dB = unity.
 * Range matches the native preview and recording clamps on the Swift side.
 *
 * The slider is intentionally a controlled component: the owning settings
 * store owns the value so the gain is shared across the live meters, the recording
 * command, and any other UI that wants to show it.
 */
export interface MicGainSliderProps {
  /** Current gain in decibels (clamped to the allowed range). */
  value: number;
  /** Called whenever the user moves the slider. */
  onChange: (next: number) => void;
  /** Optional peak readout (dBFS, post-gain) to colour the scale. */
  peakDb?: number | null;
  /** Disable the control while a recording is in flight, for example. */
  disabled?: boolean;
  /** Optional className for the wrapping container. */
  className?: string;
}

const STEP_DB = 1;

export const MicGainSlider: React.FC<MicGainSliderProps> = ({
  value,
  onChange,
  peakDb,
  disabled = false,
  className,
}) => {
  const min = MIC_GAIN_RANGE.min;
  const max = MIC_GAIN_RANGE.max;
  const safeValue = Math.max(min, Math.min(max, value));

  const reset = () => onChange(0);
  const sign = safeValue > 0 ? "+" : "";
  const formatted = `${sign}${safeValue} dB`;

  // The slider track visualises where 0 dB is, the user range, and the
  // current peak (if provided) as a thin amber line.
  const valuePct = ((safeValue - min) / (max - min)) * 100;
  const unityPct = ((0 - min) / (max - min)) * 100;
  const peakPct =
    peakDb !== null && peakDb !== undefined && Number.isFinite(peakDb)
      ? Math.max(0, Math.min(100, 50 + (peakDb / 24) * 50))
      : null;

  return (
    <div className={className}>
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[11px] font-semibold text-studio-400 uppercase tracking-wider">
          Mic Gain
        </span>
        <div className="flex items-center space-x-1.5">
          <span
            className={`text-[11px] font-mono ${
              safeValue === 0
                ? "text-studio-400"
                : safeValue > 0
                  ? "text-amber-300"
                  : "text-sky-300"
            }`}
          >
            {formatted}
          </span>
          <button
            type="button"
            onClick={reset}
            disabled={disabled || safeValue === 0}
            className="p-0.5 rounded text-studio-400 hover:text-amber-300 disabled:text-studio-600 disabled:cursor-not-allowed transition-colors"
            title="Reset to 0 dB (unity)"
          >
            <RotateCcw className="w-3 h-3" />
          </button>
        </div>
      </div>
      <div className="relative h-6 flex items-center">
        {/* Track */}
        <div className="absolute inset-x-0 h-1.5 rounded-full bg-studio-800/80 border border-studio-700/70" />
        {/* Filled portion */}
        <div
          className={`absolute h-1.5 rounded-full ${
            safeValue >= 0
              ? "bg-amber-500/80 right-1/2"
              : "bg-sky-500/80 left-1/2"
          }`}
          style={{
            width: `${Math.abs(safeValue) / max * 50}%`,
            ...(safeValue >= 0 ? { left: "50%" } : { right: "50%" }),
          }}
        />
        {/* Unity tick */}
        <div
          className="absolute h-3 w-px bg-studio-500/80"
          style={{ left: `${unityPct}%` }}
          aria-hidden="true"
        />
        {/* Peak indicator */}
        {peakPct !== null && (
          <div
            className="absolute h-3 w-0.5 bg-amber-300/80"
            style={{ left: `${peakPct}%` }}
            aria-hidden="true"
            title={`Live peak: ${peakDb?.toFixed(1)} dBFS`}
          />
        )}
        <input
          type="range"
          min={min}
          max={max}
          step={STEP_DB}
          value={safeValue}
          disabled={disabled}
          onChange={(event) => onChange(Number(event.target.value))}
          className="absolute inset-0 w-full h-full opacity-0 cursor-pointer disabled:cursor-not-allowed"
          aria-label="Microphone gain in decibels"
        />
        {/* Thumb visual (sits above the transparent input) */}
        <div
          className="absolute w-3 h-3 -ml-1.5 rounded-full bg-white shadow-md shadow-studio-950/50 pointer-events-none"
          style={{ left: `${valuePct}%` }}
          aria-hidden="true"
        />
      </div>
      <div className="flex justify-between text-[9px] font-mono text-studio-500 mt-1 px-0.5">
        <span>{min} dB</span>
        <span>0 dB</span>
        <span>+{max} dB</span>
      </div>
    </div>
  );
};
