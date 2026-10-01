import type { CameraFormat } from "./types";

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

/** Approximate file size of a video track at `mbps`, in MB per minute. */
export function megabytesPerMinute(mbps: number): number {
  return Math.round((mbps * 60) / 8);
}

/* Camera video is recorded as its own track with its own settings. */

export const CAMERA_RESOLUTIONS = ["480p", "720p", "1080p"] as const;
export type CameraResolution = (typeof CAMERA_RESOLUTIONS)[number];

export const CAMERA_FRAME_RATES = [24, 30, 60] as const;

/** Camera video bitrates, in Mbps. */
export const CAMERA_BITRATES_MBPS = [4, 6, 10] as const;

const CAMERA_SIZES: Record<CameraResolution, { width: number; height: number }> = {
  "480p": { width: 854, height: 480 },
  "720p": { width: 1280, height: 720 },
  "1080p": { width: 1920, height: 1080 },
};

export function cameraResolutionSize(resolution: CameraResolution) {
  return CAMERA_SIZES[resolution];
}

export interface CameraOptions {
  resolutions: CameraResolution[];
  frameRates: number[];
}

/** Whether a native mode can deliver `height` lines at `fps` (29.97 counts as 30). */
function delivers(format: CameraFormat, height: number, fps = 0) {
  return format.height >= height && format.fps + 1 >= fps;
}

/**
 * The camera settings worth offering: those the camera delivers natively, so
 * nothing is upscaled. With unknown formats, every setting is offered.
 */
export function cameraOptions(formats: CameraFormat[] | undefined, resolution: CameraResolution): CameraOptions {
  if (!formats || formats.length === 0) {
    return { resolutions: [...CAMERA_RESOLUTIONS], frameRates: [...CAMERA_FRAME_RATES] };
  }
  const resolutions = CAMERA_RESOLUTIONS.filter((r) =>
    formats.some((f) => delivers(f, CAMERA_SIZES[r].height)),
  );
  const offered = resolutions.length > 0 ? resolutions : [CAMERA_RESOLUTIONS[0]];
  const height = CAMERA_SIZES[closest(offered, resolution, CAMERA_RESOLUTIONS)].height;
  const frameRates = CAMERA_FRAME_RATES.filter((fps) => formats.some((f) => delivers(f, height, fps)));
  return { resolutions: offered, frameRates: frameRates.length > 0 ? frameRates : [CAMERA_FRAME_RATES[0]] };
}

/** The chosen value if offered, else the best offered below it, else the lowest offered. */
function closest<T>(offered: readonly T[], chosen: T, order: readonly T[]): T {
  if (offered.includes(chosen)) return chosen;
  const rank = order.indexOf(chosen);
  const below = offered.filter((value) => order.indexOf(value) < rank);
  return below.length > 0 ? below[below.length - 1] : offered[0];
}

/** The chosen camera quality, lowered to what this camera can deliver. */
export function effectiveCameraQuality(
  formats: CameraFormat[] | undefined,
  quality: CameraQuality,
): CameraQuality {
  const { resolutions } = cameraOptions(formats, quality.resolution);
  const resolution = closest(resolutions, quality.resolution, CAMERA_RESOLUTIONS);
  const { frameRates } = cameraOptions(formats, resolution);
  return { ...quality, resolution, fps: closest(frameRates, quality.fps, CAMERA_FRAME_RATES) };
}

/** "1080p at 30 fps", the best this camera can do. */
export function cameraMaximum(formats: CameraFormat[] | undefined): string | undefined {
  if (!formats || formats.length === 0) return undefined;
  const { resolutions } = cameraOptions(formats, "1080p");
  const best = resolutions[resolutions.length - 1];
  const { frameRates } = cameraOptions(formats, best);
  return `${best} at ${frameRates[frameRates.length - 1]} fps`;
}

export interface CameraQuality {
  resolution: CameraResolution;
  fps: number;
  bitrateMbps: number;
}

export const DEFAULT_CAMERA_QUALITY: CameraQuality = { resolution: "720p", fps: 30, bitrateMbps: 6 };

const CAMERA_QUALITY_KEY = "aeroshoot.cameraQuality";

export function loadCameraQuality(): CameraQuality {
  try {
    const raw = JSON.parse(localStorage.getItem(CAMERA_QUALITY_KEY) ?? "null") as Partial<CameraQuality> | null;
    return {
      resolution: pick<CameraResolution>(CAMERA_RESOLUTIONS, raw?.resolution, DEFAULT_CAMERA_QUALITY.resolution),
      fps: pick<number>(CAMERA_FRAME_RATES, raw?.fps, DEFAULT_CAMERA_QUALITY.fps),
      bitrateMbps: pick<number>(CAMERA_BITRATES_MBPS, raw?.bitrateMbps, DEFAULT_CAMERA_QUALITY.bitrateMbps),
    };
  } catch {
    return { ...DEFAULT_CAMERA_QUALITY };
  }
}

export function saveCameraQuality({ resolution, fps, bitrateMbps }: CameraQuality) {
  try {
    localStorage.setItem(CAMERA_QUALITY_KEY, JSON.stringify({ resolution, fps, bitrateMbps }));
  } catch {
    // Storage can be unavailable; the choice still applies for this session.
  }
}
