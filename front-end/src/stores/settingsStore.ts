import { create } from "zustand";
import {
  CameraBubbleSettings,
  CanvasSettings,
  CaptureSource,
  CameraDevice,
  AudioDevice,
} from "../lib/types";

interface SettingsStore {
  selectedSourceId: string | null;
  selectedCameraId: string | null;
  selectedMicId: string | null;
  captureSystemAudio: boolean;
  fps: number;
  resolution: "1080p" | "4K";
  cameraBubble: CameraBubbleSettings;
  canvas: CanvasSettings;
  activeScene: "record" | "edit";
  selectionsReady: boolean;

  setActiveScene: (scene: "record" | "edit") => void;
  setSelectedSourceId: (id: string | null) => void;
  setSelectedCameraId: (id: string | null) => void;
  setSelectedMicId: (id: string | null) => void;
  setCaptureSystemAudio: (enabled: boolean) => void;
  setFps: (fps: number) => void;
  setResolution: (res: "1080p" | "4K") => void;
  updateCameraBubble: (settings: Partial<CameraBubbleSettings>) => void;
  updateCanvas: (settings: Partial<CanvasSettings>) => void;
  reconcileSelections: (
    sources: CaptureSource[],
    cameras: CameraDevice[],
    mics: AudioDevice[],
  ) => void;
}

export const useSettingsStore = create<SettingsStore>((set, get) => ({
  selectedSourceId: null,
  selectedCameraId: null,
  selectedMicId: null,
  captureSystemAudio: true,
  fps: 30,
  resolution: "1080p",
  activeScene: "record",
  selectionsReady: false,

  setActiveScene: (scene) => set({ activeScene: scene }),

  cameraBubble: {
    enabled: true,
    shape: "squircle",
    size: "md",
    position: "bottom-right",
    customX: 80,
    customY: 80,
    borderColor: "#6366f1",
    borderWidth: 3,
    mirror: true,
    shadow: true,
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
  setFps: (fps) => set({ fps }),
  setResolution: (resolution) => set({ resolution }),
  updateCameraBubble: (settings) =>
    set((state) => ({ cameraBubble: { ...state.cameraBubble, ...settings } })),
  updateCanvas: (settings) =>
    set((state) => ({ canvas: { ...state.canvas, ...settings } })),

  /**
   * Reconcile persistent selection IDs against the freshly enumerated native
   * source/device lists. Preserves an ID only if it still exists; otherwise
   * falls back to the first available display / a device marked `isDefault`,
   * or `null` for optional devices (mic) when nothing matches.
   *
   * Once this runs successfully the store becomes `selectionsReady`, which
   * gates the record button in the HUD.
   */
  reconcileSelections: (sources, cameras, mics) => {
    const state = get();
    // Native enumeration is the source of truth for IDs. Keep malformed
    // entries out of the selection sets so a stale/partial bridge response
    // can never make an arbitrary persisted ID look valid.
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
        : safeSources.find((s) => s.sourceType === "display")?.id ?? safeSources[0]?.id ?? null;

    const nextCameraId =
      state.selectedCameraId && cameraIds.has(state.selectedCameraId)
        ? state.selectedCameraId
        : safeCameras.find((c) => c.isDefault)?.id ?? safeCameras[0]?.id ?? null;

    const nextMicId =
      state.selectedMicId && micIds.has(state.selectedMicId)
        ? state.selectedMicId
        : safeMics.find((m) => m.isDefault)?.id ?? safeMics[0]?.id ?? null;

    set({
      selectedSourceId: nextSourceId,
      selectedCameraId: nextCameraId,
      selectedMicId: nextMicId,
      cameraBubble: {
        ...state.cameraBubble,
        enabled: safeCameras.length === 0 ? false : state.cameraBubble.enabled,
      },
      // Reconciliation is complete only after all three native enumeration
      // responses have settled. An empty but valid response is still ready;
      // recording remains disabled because there is no source to select.
      selectionsReady:
        Array.isArray(sources) && Array.isArray(cameras) && Array.isArray(mics),
    });
  },
}));
