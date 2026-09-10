import { create } from "zustand";
import {
  CameraBubbleSettings,
  CanvasSettings,
  CaptureSource,
  CameraDevice,
  AudioDevice,
  EditLayout,
  HudSnapshot,
  HudCameraInfo,
  canvasFromLayout,
  cameraFromLayout,
} from "../lib/types";
import { api } from "../lib/ipc";

// The studio and camera HUD share a revisioned native settings owner. Inspector
// controls can emit many changes in a single frame (notably range inputs), so
// serialize writes instead of racing every patch with the same revision.
let hudUpdateQueue: Promise<void> = Promise.resolve();
let pendingHudPatches: Partial<CameraBubbleSettings>[] = [];

function queueHudPatch(patch: Partial<CameraBubbleSettings>) {
  pendingHudPatches.push(patch);
  const removePending = () => {
    const index = pendingHudPatches.indexOf(patch);
    if (index >= 0) pendingHudPatches.splice(index, 1);
  };
  hudUpdateQueue = hudUpdateQueue.catch(() => undefined).then(async () => {
    let snapshot;
    try {
      snapshot = await api.hudUpdate(useSettingsStore.getState().hudRevision, patch);
    } catch {
      // A change from the other window may legitimately advance the owner.
      // Reconcile once and retry this patch against the current revision.
      const current = await api.hudSnapshot();
      useSettingsStore.getState().applyHudSnapshot(current);
      snapshot = await api.hudUpdate(current.revision, patch);
    }
    removePending();
    useSettingsStore.getState().applyHudSnapshot(snapshot);
  }).catch(async () => {
    removePending();
    // Do not leave a permanently optimistic patch behind if the native owner
    // is unavailable; restore the last authoritative snapshot when possible.
    try {
      useSettingsStore.getState().applyHudSnapshot(await api.hudSnapshot());
    } catch {
      // The local setting remains usable when running outside the desktop app.
    }
  });
}

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
  projectName: string;
  projectDir: string | null;
  createdProjectPath: string | null;
  hudRevision: number;
  hudCameraAvailable: boolean;
  hudDiagnostics: string[];
  knownCameras: HudCameraInfo[];
  layoutOwnedByProject: boolean;

  setActiveScene: (scene: "record" | "edit") => void;
  setProjectName: (name: string) => void;
  setProjectDir: (dir: string | null) => void;
  setCreatedProjectPath: (path: string | null) => void;
  setSelectedSourceId: (id: string | null) => void;
  setSelectedCameraId: (id: string | null) => void;
  setSelectedMicId: (id: string | null) => void;
  setCaptureSystemAudio: (enabled: boolean) => void;
  setFps: (fps: number) => void;
  setResolution: (res: "1080p" | "4K") => void;
  updateCameraBubble: (settings: Partial<CameraBubbleSettings>) => void;
  updateCanvas: (settings: Partial<CanvasSettings>) => void;
  hydrateLayout: (layout: EditLayout) => void;
  applyHudSnapshot: (snapshot: HudSnapshot) => void;
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
  projectName: "",
  projectDir: null,
  createdProjectPath: null,
  hudRevision: 0,
  hudCameraAvailable: true,
  hudDiagnostics: [],
  knownCameras: [],
  layoutOwnedByProject: false,

  setActiveScene: (scene) => set({ activeScene: scene }),
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
  setSelectedCameraId: (id) => {
    set({ selectedCameraId: id });
    const cameras = get().knownCameras;
    void api
      .hudReconcileCameras(cameras, id)
      .then((snapshot) => get().applyHudSnapshot(snapshot))
      .catch(() => undefined);
  },
  setSelectedMicId: (id) => set({ selectedMicId: id }),
  setCaptureSystemAudio: (enabled) => set({ captureSystemAudio: enabled }),
  setFps: (fps) => set({ fps }),
  setResolution: (resolution) => set({ resolution }),
  updateCameraBubble: (settings) => {
    set((state) => ({ cameraBubble: { ...state.cameraBubble, ...settings } }));
    queueHudPatch(settings);
  },
  updateCanvas: (settings) =>
    set((state) => ({ canvas: { ...state.canvas, ...settings } })),
  hydrateLayout: (layout) =>
    set({
      canvas: canvasFromLayout(layout),
      cameraBubble: cameraFromLayout(layout),
      layoutOwnedByProject: true,
    }),
  applyHudSnapshot: (snapshot) =>
    set((state) => {
      const next: Partial<SettingsStore> = {
        hudRevision: snapshot.revision,
        hudCameraAvailable: snapshot.cameraAvailable,
        hudDiagnostics: snapshot.diagnostics,
      };
      if (snapshot.cameraId) {
        next.selectedCameraId = snapshot.cameraId;
      }
      if (!state.layoutOwnedByProject) {
        let cameraBubble: CameraBubbleSettings = {
          ...state.cameraBubble,
          enabled: snapshot.settings.enabled,
          shape: snapshot.settings.shape,
          size: snapshot.settings.size,
          mirror: snapshot.settings.mirror,
          borderColor: snapshot.settings.borderColor,
          borderWidth: snapshot.settings.borderWidth,
          shadow: snapshot.settings.shadow,
        };
        // Native events and command responses can arrive before later queued
        // writes. Keep the optimistic UI (and recording options) at the newest
        // requested values until those writes have committed.
        for (const patch of pendingHudPatches) {
          cameraBubble = { ...cameraBubble, ...patch };
        }
        next.cameraBubble = cameraBubble;
      }
      return next;
    }),

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
      knownCameras: safeCameras.map((camera) => ({ id: camera.id, name: camera.name })),
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
