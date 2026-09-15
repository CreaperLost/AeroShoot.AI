import { create } from "zustand";
import {
  CameraBubbleSettings,
  CanvasSettings,
  CaptureSource,
  CameraDevice,
  AudioDevice,
  EditLayout,
  canvasFromLayout,
  cameraFromLayout,
} from "../lib/types";
import { loadRecordingQuality, Resolution, saveRecordingQuality } from "../lib/recordingQuality";

interface SettingsStore {
  selectedSourceId: string | null;
  availableSourceFallbackId: string | null;
  selectedCameraId: string | null;
  selectedMicId: string | null;
  captureSystemAudio: boolean;
  fps: number;
  resolution: Resolution;
  /** Screen video bitrate in Mbps (10, 20 or 30). */
  videoBitrateMbps: number;
  cameraBubble: CameraBubbleSettings;
  canvas: CanvasSettings;
  selectionsReady: boolean;
  projectName: string;
  projectDir: string | null;
  createdProjectPath: string | null;
  layoutOwnedByProject: boolean;
  /// Microphone gain in decibels applied to the mic track on the native
  /// side and shown on the live meters. Clamped to ±24 dB; 0 = unity.
  micGainDb: number;
  /** Log pointer motion and clicks during screen recordings. On by default. */
  captureMouse: boolean;

  setProjectName: (name: string) => void;
  setProjectDir: (dir: string | null) => void;
  setCreatedProjectPath: (path: string | null) => void;
  setSelectedSourceId: (id: string | null) => void;
  setSelectedCameraId: (id: string | null) => void;
  setSelectedMicId: (id: string | null) => void;
  setCaptureSystemAudio: (enabled: boolean) => void;
  setFps: (fps: number) => void;
  setResolution: (res: Resolution) => void;
  setVideoBitrateMbps: (mbps: number) => void;
  setMicGainDb: (gainDb: number) => void;
  setCaptureMouse: (enabled: boolean) => void;
  updateCameraBubble: (settings: Partial<CameraBubbleSettings>) => void;
  updateCanvas: (settings: Partial<CanvasSettings>) => void;
  hydrateLayout: (layout: EditLayout) => void;
  reconcileSelections: (
    sources: CaptureSource[],
    cameras: CameraDevice[],
    mics: AudioDevice[],
  ) => void;
}

const initialQuality = loadRecordingQuality();

const CAPTURE_MOUSE_KEY = "aeroshoot.captureMouse";

function loadCaptureMouse(): boolean {
  try {
    return localStorage.getItem(CAPTURE_MOUSE_KEY) !== "false";
  } catch {
    return true;
  }
}

function saveCaptureMouse(enabled: boolean) {
  try {
    localStorage.setItem(CAPTURE_MOUSE_KEY, String(enabled));
  } catch {
    // Storage can be unavailable; the choice still applies for this session.
  }
}

export const useSettingsStore = create<SettingsStore>((set, get) => ({
  selectedSourceId: null,
  availableSourceFallbackId: null,
  selectedCameraId: null,
  selectedMicId: null,
  captureSystemAudio: true,
  fps: initialQuality.fps,
  resolution: initialQuality.resolution,
  videoBitrateMbps: initialQuality.videoBitrateMbps,
  selectionsReady: false,
  projectName: "",
  projectDir: null,
  createdProjectPath: null,
  layoutOwnedByProject: false,
  micGainDb: 0,
  captureMouse: loadCaptureMouse(),

  setProjectName: (projectName) => set({ projectName }),
  setProjectDir: (projectDir) => set({ projectDir }),
  setCreatedProjectPath: (createdProjectPath) => set({ createdProjectPath }),

  cameraBubble: {
    enabled: true,
    shape: "rect",
    size: "md",
    position: "bottom-right",
    customX: 80,
    customY: 80,
    borderColor: "#6366f1",
    borderWidth: 3,
    mirror: true,
    shadow: false,
  },

  canvas: {
    backgroundType: "gradient",
    colorStart: "#312e81",
    colorEnd: "#0f172a",
    paddingPx: 32,
    cornerRadiusPx: 16,
    shadowBlurPx: 24,
    shadowOpacity: 0.5,
    aspectRatio: "16:9",
  },

  setSelectedSourceId: (id) => set({ selectedSourceId: id }),
  setSelectedCameraId: (id) => set({ selectedCameraId: id }),
  setSelectedMicId: (id) => set({ selectedMicId: id }),
  setCaptureSystemAudio: (enabled) => set({ captureSystemAudio: enabled }),
  setFps: (fps) => {
    set({ fps });
    saveRecordingQuality(get());
  },
  setResolution: (resolution) => {
    set({ resolution });
    saveRecordingQuality(get());
  },
  setVideoBitrateMbps: (videoBitrateMbps) => {
    set({ videoBitrateMbps });
    saveRecordingQuality(get());
  },
  setMicGainDb: (micGainDb) => {
    const clamped = Math.max(-24, Math.min(24, micGainDb));
    set({ micGainDb: clamped });
  },
  setCaptureMouse: (captureMouse) => {
    set({ captureMouse });
    saveCaptureMouse(captureMouse);
  },
  updateCameraBubble: (settings) =>
    set((state) => ({ cameraBubble: { ...state.cameraBubble, ...settings } })),
  updateCanvas: (settings) =>
    set((state) => ({ canvas: { ...state.canvas, ...settings } })),
  hydrateLayout: (layout) =>
    set({
      canvas: canvasFromLayout(layout),
      cameraBubble: cameraFromLayout(layout),
      layoutOwnedByProject: true,
    }),

  reconcileSelections: (sources, cameras, mics) => {
    const state = get();
    const safeSources = Array.isArray(sources)
      ? sources.filter((source) => typeof source?.id === "string" && source.id.length > 0)
      : [];
    const safeCameras = Array.isArray(cameras)
      ? cameras.filter((camera) => typeof camera?.id === "string" && camera.id.length > 0)
      : [];
    const safeMics = Array.isArray(mics)
      ? mics.filter((mic) => typeof mic?.id === "string" && mic.id.length > 0)
      : [];

    const sourceIds = new Set(safeSources.map((s) => s.id));
    const cameraIds = new Set(safeCameras.map((c) => c.id));
    const micIds = new Set(safeMics.map((m) => m.id));

    const nextSourceId =
      state.selectedSourceId && sourceIds.has(state.selectedSourceId)
        ? state.selectedSourceId
        : state.selectionsReady && state.selectedSourceId === null
          ? null
          : safeSources.find((s) => s.sourceType === "display")?.id ?? safeSources[0]?.id ?? null;

    const nextCameraId =
      state.selectedCameraId && cameraIds.has(state.selectedCameraId)
        ? state.selectedCameraId
        : safeCameras.find((c) => c.isDefault)?.id ?? safeCameras[0]?.id ?? null;

    const nextMicId =
      state.selectedMicId && micIds.has(state.selectedMicId)
        ? state.selectedMicId
        : state.selectionsReady && state.selectedMicId === null
          ? null
          : safeMics.find((m) => m.isDefault)?.id ?? safeMics[0]?.id ?? null;

    set({
      selectedSourceId: nextSourceId,
      availableSourceFallbackId: safeSources.find((s) => s.sourceType === "display")?.id ?? safeSources[0]?.id ?? null,
      selectedCameraId: nextCameraId,
      selectedMicId: nextMicId,
      cameraBubble: {
        ...state.cameraBubble,
        enabled: safeCameras.length === 0 ? false : state.cameraBubble.enabled,
      },
      selectionsReady:
        Array.isArray(sources) && Array.isArray(cameras) && Array.isArray(mics),
    });
  },
}));
