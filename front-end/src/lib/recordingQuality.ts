export const RESOLUTIONS = ["720p", "1080p", "1440p", "4K"] as const;
export type Resolution = (typeof RESOLUTIONS)[number];

export const FRAME_RATES = [24, 30, 60] as const;

/** Screen video bitrates offered by the recorder, in Mbps. */
export const BITRATES_MBPS = [10, 20, 30] as const;

const SIZES: Record<Resolution, { width: number; height: number }> = {
  "720p": { width: 1280, height: 720 },
  "1080p": { width: 1920, height: 1080 },
  "1440p": { width: 2560, height: 1440 },
  "4K": { width: 3840, height: 2160 },
};

export function resolutionSize(resolution: Resolution) {
  return SIZES[resolution];
}

export interface RecordingQuality {
  fps: number;
  resolution: Resolution;
  videoBitrateMbps: number;
}

export const DEFAULT_QUALITY: RecordingQuality = { fps: 30, resolution: "1080p", videoBitrateMbps: 20 };

const QUALITY_KEY = "aeroshoot.recordingQuality";

const pick = <T>(allowed: readonly T[], value: unknown, fallback: T): T =>
  (allowed as readonly unknown[]).includes(value) ? (value as T) : fallback;

export function loadRecordingQuality(): RecordingQuality {
  try {
    const raw = JSON.parse(localStorage.getItem(QUALITY_KEY) ?? "null") as Partial<RecordingQuality> | null;
    return {
      fps: pick<number>(FRAME_RATES, raw?.fps, DEFAULT_QUALITY.fps),
      resolution: pick<Resolution>(RESOLUTIONS, raw?.resolution, DEFAULT_QUALITY.resolution),
      videoBitrateMbps: pick<number>(BITRATES_MBPS, raw?.videoBitrateMbps, DEFAULT_QUALITY.videoBitrateMbps),
    };
  } catch {
    return { ...DEFAULT_QUALITY };
  }
}

export function saveRecordingQuality({ fps, resolution, videoBitrateMbps }: RecordingQuality) {
  try {
    localStorage.setItem(QUALITY_KEY, JSON.stringify({ fps, resolution, videoBitrateMbps }));
  } catch {
    // Storage can be unavailable; the choice still applies for this session.
  }
}
