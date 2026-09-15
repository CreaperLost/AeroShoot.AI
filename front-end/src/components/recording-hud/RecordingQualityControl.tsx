import { useSettingsStore } from "../../stores/settingsStore";
import {
  BITRATES_MBPS,
  FRAME_RATES,
  Resolution,
  RESOLUTIONS,
  resolutionSize,
} from "../../lib/recordingQuality";

export function Segmented<T>({
  label,
  unit,
  options,
  value,
  onChange,
  format,
  disabled,
}: {
  label: string;
  unit?: string;
  options: readonly T[];
  value: T;
  onChange: (value: T) => void;
  format: (value: T) => string;
  disabled: boolean;
}) {
  return (
    <div>
      <div className="mb-1 flex items-baseline justify-between text-[10px] font-semibold uppercase tracking-wider text-studio-400">
        <span>{label}</span>
        {unit && <span className="font-mono normal-case text-studio-500">{unit}</span>}
      </div>
      <div
        role="radiogroup"
        aria-label={label}
        className="grid gap-0.5 rounded-lg border border-studio-800 bg-studio-950/60 p-0.5"
        style={{ gridTemplateColumns: `repeat(${options.length}, minmax(0, 1fr))` }}
      >
        {options.map((option) => {
          const selected = option === value;
          return (
            <button
              key={String(option)}
              type="button"
              role="radio"
              aria-checked={selected}
              disabled={disabled}
              onClick={() => onChange(option)}
              className={`rounded-md px-1 py-1 font-mono text-[11px] transition-colors disabled:opacity-50 ${
                selected ? "bg-studio-800 text-white shadow-sm" : "text-studio-400 hover:text-studio-200"
              }`}
            >
              {format(option)}
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function RecordingQualityControl({ disabled }: { disabled: boolean }) {
  const { fps, setFps, resolution, setResolution, videoBitrateMbps, setVideoBitrateMbps } = useSettingsStore();
  const { width, height } = resolutionSize(resolution);
  const megabytesPerMinute = Math.round((videoBitrateMbps * 60) / 8);

  return (
    <div className="space-y-2.5">
      <Segmented<Resolution>
        label="Resolution"
        options={RESOLUTIONS}
        value={resolution}
        onChange={setResolution}
        format={(value) => value}
        disabled={disabled}
      />
      <Segmented<number>
        label="Frame rate"
        unit="fps"
        options={FRAME_RATES}
        value={fps}
        onChange={setFps}
        format={(value) => String(value)}
        disabled={disabled}
      />
      <Segmented<number>
        label="Bitrate"
        unit="Mbps"
        options={BITRATES_MBPS}
        value={videoBitrateMbps}
        onChange={setVideoBitrateMbps}
        format={(value) => String(value)}
        disabled={disabled}
      />
      <p className="text-[10px] leading-snug text-studio-500">
        {width}×{height} · {fps} fps · {videoBitrateMbps} Mbps · ≈{megabytesPerMinute} MB/min of screen video
      </p>
    </div>
  );
}
