import {
  OpenedProject,
  SegmentPage,
  StopRecordingResult,
  WaveformPage,
  PlaybackStatus,
  EditCut,
  PreviewStatus,
  PreviewViewport,
  PreviewHitMode,
  MediaInteropStatus,
  MediaParityReport,
  ExportSettings,
  ExportStatus,
  CaptureSource,
  CameraDevice,
  AudioDevice,
  SessionState,
  SilenceConfig,
  SilenceDetectionResult,
  PermissionBundle,
  PermissionState,
  MouseTelemetryPermission,
  ZoomGeneration,
  ZoomConfig,
  ProjectZoom,
  ManualZoomInput,
  EditLayout,
  WindowIdentity,
  HudSnapshot,
  HudSettingsPatch,
  HudCameraInfo,
} from "./types";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
    __TAURI__?: unknown;
  }
}

export const isTauriEnvironment = (): boolean => {
  return typeof window !== "undefined" && (Boolean(window.__TAURI_INTERNALS__) || Boolean(window.__TAURI__));
};

// Generic invoker that uses Tauri invoke when available or falls back to synthetic data
async function invokeTauri<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauriEnvironment()) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      return await invoke<T>(cmd, args);
    } catch (err) {
      console.warn(`[Tauri IPC] Failed invoking ${cmd}:`, err);
      throw err;
    }
  }

  // Emulated synthetic backend for browser dev environment
  return emulateCommand<T>(cmd, args);
}

const ALL_AUTHORIZED: PermissionBundle = {
  screenRecording: "authorized",
  camera: "authorized",
  microphone: "authorized",
};

/**
 * Best-effort normalization from whatever the native bridge returns into our
 * typed `PermissionBundle`. Today the Rust side returns three booleans; once
 * it returns the typed bundle this function is a no-op.
 */
function normalizePermissionBundle(raw: unknown): PermissionBundle {
  if (!raw || typeof raw !== "object") {
    return { screenRecording: "unknown", camera: "unknown", microphone: "unknown" };
  }
  const r = raw as Record<string, unknown>;

  const toState = (value: unknown): PermissionState => {
    if (typeof value === "string") {
      switch (value) {
        case "notDetermined":
        case "authorized":
        case "denied":
        case "restricted":
        case "unknown":
          return value;
      }
    }
    if (typeof value === "boolean") {
      return value ? "authorized" : "denied";
    }
    if (typeof value === "number") {
      switch (value) {
        case 1:
          return "notDetermined";
        case 2:
          return "authorized";
        case 3:
          return "denied";
        case 4:
          return "restricted";
        default:
          return "unknown";
      }
    }
    return "unknown";
  };

  return {
    screenRecording: toState(r.screenRecording ?? r.screen_recording),
    camera: toState(r.camera),
    microphone: toState(r.microphone),
  };
}

/**
 * Mouse telemetry is a boolean pair. Do not run it through
 * `normalizePermissionBundle` — that path is screen/camera/microphone.
 */
function normalizeMouseTelemetryPermission(raw: unknown): MouseTelemetryPermission {
  if (!raw || typeof raw !== "object") {
    return { supported: false, authorized: false };
  }
  const r = raw as Record<string, unknown>;
  if ("screenRecording" in r || "screen_recording" in r) {
    return { supported: false, authorized: false };
  }
  return {
    supported: r.supported === true,
    authorized: r.authorized === true,
  };
}

export function getRealDisplays(): CaptureSource[] {
  if (typeof window === "undefined" || typeof window.screen === "undefined") {
    return [];
  }

  const dpr = window.devicePixelRatio || 1;
  const width = Math.round(window.screen.width * dpr);
  const height = Math.round(window.screen.height * dpr);

  return [
    {
      id: "display-primary",
      name: `Main Display (${width} × ${height})`,
      sourceType: "display",
      width: width || 1920,
      height: height || 1080,
    },
  ];
}

export async function getBrowserDevices(): Promise<{ cameras: CameraDevice[]; mics: AudioDevice[] }> {
  try {
    if (typeof navigator !== "undefined" && navigator.mediaDevices?.enumerateDevices) {
      let devices = await navigator.mediaDevices.enumerateDevices();

      // Modern browsers conceal hardware device labels until media permissions are requested.
      // If hardware devices are present but have blank labels, trigger a silent probe to reveal real OS names.
      const hasBlankLabels = devices.some(
        (d) => (d.kind === "videoinput" || d.kind === "audioinput") && !d.label
      );

      if (hasBlankLabels && navigator.mediaDevices.getUserMedia) {
        try {
          const probeStream = await navigator.mediaDevices
            .getUserMedia({ video: true, audio: true })
            .catch(async () => {
              return await navigator.mediaDevices.getUserMedia({ video: true }).catch(async () => {
                return await navigator.mediaDevices.getUserMedia({ audio: true }).catch(() => null);
              });
            });

          if (probeStream) {
            probeStream.getTracks().forEach((track) => track.stop());
            devices = await navigator.mediaDevices.enumerateDevices();
          }
        } catch (permErr) {
          console.warn("[IPC] Media permissions probe skipped or denied:", permErr);
        }
      }

      // Filter and deduplicate real camera hardware
      const rawCameras = devices.filter((d) => d.kind === "videoinput");
      const cameras: CameraDevice[] = [];
      const seenCamIds = new Set<string>();

      rawCameras.forEach((d, idx) => {
        const id = d.deviceId || `camera-${idx}`;
        if (seenCamIds.has(id)) return;
        seenCamIds.add(id);

        let name = d.label.trim();
        if (!name) {
          name = idx === 0 ? "Default Camera (Built-in)" : `Camera ${idx + 1}`;
        }
        cameras.push({
          id,
          name,
          isDefault: idx === 0,
        });
      });

      // Filter and deduplicate real microphone hardware
      const rawMics = devices.filter((d) => d.kind === "audioinput");
      const mics: AudioDevice[] = [];
      const seenMicIds = new Set<string>();

      rawMics.forEach((d, idx) => {
        const id = d.deviceId || `mic-${idx}`;
        if (seenMicIds.has(id)) return;
        seenMicIds.add(id);

        let name = d.label.trim();
        if (!name) {
          name = idx === 0 ? "Default Microphone" : `Microphone ${idx + 1}`;
        }
        mics.push({
          id,
          name,
          isDefault: idx === 0,
        });
      });

      return { cameras, mics };
    }
  } catch (e) {
    console.warn("[IPC] Browser device enumeration error:", e);
  }
  return { cameras: [], mics: [] };
}

// Emulation layer
let mockSessionState: SessionState = "idle";
let mockSessionStartUs = 0;
let mockHudRevision = 0;
let mockHud: HudSnapshot = {
  revision: 0,
  settings: {
    enabled: true,
    shape: "circle",
    size: "md",
    mirror: true,
    borderColor: "#6366f1",
    borderWidth: 3,
    shadow: false,
  },
  cameraId: null,
  cameraName: null,
  cameraAvailable: false,
  hudAttached: false,
  hudVisible: true,
  exclusionEstablished: false,
  hideDuringRecord: true,
  sessionRecording: false,
  captureSessionAlive: false,
  startedIndependentCapture: false,
  hitMode: "circle_pass_through",
  diagnostics: [],
};

function bumpMockHud(patch: Partial<HudSnapshot> = {}): HudSnapshot {
  mockHudRevision += 1;
  mockHud = { ...mockHud, ...patch, revision: mockHudRevision };
  return mockHud;
}

async function emulateCommand<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  console.log(`[Synthetic Backend] ${cmd}`, args);

  switch (cmd) {
    case "list_capture_sources":
      return Promise.resolve(getRealDisplays() as unknown as T);

    case "list_devices": {
      const devices = await getBrowserDevices();
      return devices as unknown as T;
    }

    case "start_recording":
      mockSessionState = "recording";
      mockSessionStartUs = Date.now() * 1000;
      return Promise.resolve({
        sessionId: "sess-" + Math.random().toString(36).substring(2, 9),
        state: mockSessionState,
        startedAtUs: mockSessionStartUs,
      } as unknown as T);

    case "pause_recording":
      mockSessionState = "paused";
      return Promise.resolve({ state: mockSessionState } as unknown as T);

    case "resume_recording":
      mockSessionState = "recording";
      return Promise.resolve({ state: mockSessionState } as unknown as T);

    case "capture_preview_configure":
      return Promise.resolve(undefined as unknown as T);

    case "open_project":
    case "close_project":
    case "project_rename":
    case "project_segments":
    case "project_waveform":
    case "project_zoom_suggestions":
    case "project_zoom_accept":
    case "project_zoom_dismiss":
    case "project_zoom_update":
    case "project_zoom_add":
    case "project_zoom_delete":
    case "project_layout_update":
    case "project_ripple_cuts":
    case "detect_silence":
    case "project_undo":
    case "project_redo":
    case "playback_status":
    case "playback_play":
    case "playback_pause":
    case "playback_seek":
    case "preview_attach":
    case "preview_layout":
    case "preview_present_fixed":
    case "preview_present_fixture":
    case "preview_status":
    case "preview_hit_test":
    case "preview_detach":
    case "media_interop_status":
    case "media_run_parity":
    case "export_start":
    case "export_status":
    case "export_cancel":
      throw new Error("Opening real projects requires the desktop app.");

    case "stop_recording":
      mockSessionState = "completed";
      return Promise.resolve({
        projectPath: "",
        sessionId: "sess-completed",
        state: mockSessionState,
        durationUs: 0,
      } as unknown as T);

    case "get_session_status":
      return Promise.resolve({
        state: mockSessionState,
        elapsedUs: mockSessionState === "recording" ? Date.now() * 1000 - mockSessionStartUs : 0,
        droppedFrames: 0,
        audioBufferUnderflows: 0,
      } as unknown as T);

    case "mouse_telemetry_permission":
      return { supported: false, authorized: false } as T;

    case "get_permission_status":
      return Promise.resolve(ALL_AUTHORIZED as unknown as T);

    case "request_capture_permissions":
      return Promise.resolve(ALL_AUTHORIZED as unknown as T);

    case "open_system_privacy_settings": {
      const pane = (args?.pane as string) || "ScreenCapture";
      if (typeof window !== "undefined") {
        try {
          window.open(`x-apple.systempreferences:com.apple.preference.security?Privacy_${pane}`);
        } catch {
          // ignore
        }
      }
      return Promise.resolve({ opened: true } as unknown as T);
    }

    case "get_default_projects_dir":
      return Promise.resolve("Documents/AeroShootRec" as unknown as T);

    case "pick_project_folder":
      throw new Error("Opening a project folder requires the desktop app.");

    case "pick_save_directory":
      throw new Error("Choosing a save location requires the desktop app.");

    case "pick_export_destination":
      throw new Error("Choosing an export destination requires the desktop app.");

    case "pick_wallpaper_source":
      throw new Error("Choosing a wallpaper file requires the desktop app.");

    case "window_identity":
      return Promise.resolve({
        label: "main",
        uiRoot: "studio",
        rejected: false,
      } as unknown as T);

    case "hud_snapshot":
      return Promise.resolve(mockHud as unknown as T);

    case "hud_update": {
      const expected = Number(args?.expectedRevision ?? -1);
      if (expected !== mockHud.revision) {
        return Promise.reject(new Error("Stale HUD settings revision"));
      }
      const patch = (args?.patch ?? {}) as HudSettingsPatch;
      const settings = { ...mockHud.settings, ...patch };
      return Promise.resolve(
        bumpMockHud({
          settings,
          hitMode:
            settings.shape === "circle"
              ? "circle_pass_through"
              : settings.shape === "squircle"
                ? "squircle_pass_through"
                : "pass_through",
        }) as unknown as T,
      );
    }

    case "hud_reconcile_cameras": {
      const cameras = (args?.cameras as HudCameraInfo[] | undefined) ?? [];
      const requestedId = args?.selectedCameraId as string | null | undefined;
      const currentId = requestedId ?? mockHud.cameraId;
      const found = currentId ? cameras.find((c) => c.id === currentId) : cameras[0];
      return Promise.resolve(
        bumpMockHud({
          cameraId: found?.id ?? currentId,
          cameraName: found?.name ?? null,
          cameraAvailable: Boolean(found),
        }) as unknown as T,
      );
    }

    case "hud_preview_attach":
      if (args?.windowLabel !== "camera_overlay") {
        return Promise.reject(new Error("HUD preview can only attach to 'camera_overlay'"));
      }
      return Promise.resolve(
        bumpMockHud({ hudAttached: true, startedIndependentCapture: false }) as unknown as T,
      );

    case "hud_preview_layout":
    case "hud_preview_status":
      return Promise.resolve({
        attached: mockHud.hudAttached,
        windowLabel: mockHud.hudAttached ? "camera_overlay" : null,
        generation: 1,
        layoutRevision: 0,
        arrangement: "child_overlay",
        supported: false,
        presentedKind: "none",
        copiesPerPresent: 1,
        copies: 0,
        presentedBytes: 0,
        backingScale: 1,
        physical: null,
        visible: mockHud.hudVisible,
        occluded: !mockHud.hudVisible,
        hitMode: mockHud.hitMode,
        diagnostics: [],
      } as unknown as T);

    case "hud_close":
      return Promise.resolve(
        bumpMockHud({
          hudAttached: false,
          hudVisible: false,
          startedIndependentCapture: false,
          captureSessionAlive: mockSessionState === "recording" || mockSessionState === "paused",
          sessionRecording: mockSessionState === "recording" || mockSessionState === "paused",
        }) as unknown as T,
      );

    case "hud_set_visible":
      return Promise.resolve(
        bumpMockHud({
          hudVisible:
            Boolean(args?.visible) &&
            mockHud.settings.enabled &&
            (mockSessionState === "recording" || mockSessionState === "paused"),
        }) as unknown as T,
      );

    case "set_window_title": {
      const title = args?.title as string | undefined;
      if (typeof document !== "undefined" && typeof title === "string") {
        document.title = title;
      }
      return Promise.resolve({} as T);
    }

    case "show_in_finder":
      console.log("[Synthetic] show_in_finder:", args?.path);
      return Promise.resolve({} as T);

    default:
      return Promise.resolve({} as T);
  }
}

// Exported high-level IPC functions
export const api = {
  listCaptureSources: async (): Promise<CaptureSource[]> => {
    try {
      if (isTauriEnvironment()) {
        const sources = await invokeTauri<CaptureSource[]>("list_capture_sources");
        if (Array.isArray(sources)) {
          // Keep strictly physical displays, filter out any mock or window sources
          const realDisplays = sources.filter((s) => s.sourceType === "display");
          if (realDisplays.length > 0) {
            return realDisplays;
          }
        }
      }
    } catch (err) {
      console.warn("[IPC] listCaptureSources native call failed:", err);
    }
    return getRealDisplays();
  },
  listDevices: async (): Promise<{ cameras: CameraDevice[]; mics: AudioDevice[] }> => {
    try {
      if (isTauriEnvironment()) {
        const devices = await invokeTauri<{ cameras: CameraDevice[]; mics: AudioDevice[] }>("list_devices");
        if (devices) {
          return {
            cameras: Array.isArray(devices.cameras) ? devices.cameras : [],
            mics: Array.isArray(devices.mics) ? devices.mics : [],
          };
        }
      }
    } catch (err) {
      console.warn("[IPC] listDevices native call failed, querying browser:", err);
    }
    return getBrowserDevices();
  },
  /**
   * Input Monitoring preflight/request. Returns `{ supported, authorized }`
   * booleans — not a capture PermissionBundle.
   */
  mouseTelemetryPermission: async (request = false): Promise<MouseTelemetryPermission> => {
    const raw = await invokeTauri<unknown>("mouse_telemetry_permission", { request });
    return normalizeMouseTelemetryPermission(raw);
  },

  getPermissionStatus: async (): Promise<PermissionBundle> => {
    const raw = await invokeTauri<unknown>("get_permission_status");
    return normalizePermissionBundle(raw);
  },
  /**
   * Triggers the macOS permission flow for screen recording, camera, and
   * microphone. Resolves once all requested permissions have reached a
   * terminal state. The HUD should call this on mount and again after the
   * user has visited System Settings so the UI can refresh.
   */
  requestCapturePermissions: async (options?: {
    screen?: boolean;
    camera?: boolean;
    microphone?: boolean;
  }): Promise<PermissionBundle> => {
    const raw = await invokeTauri<unknown>("request_capture_permissions", {
      screen: options?.screen ?? true,
      camera: options?.camera ?? false,
      microphone: options?.microphone ?? false,
    });
    return normalizePermissionBundle(raw);
  },
  /**
   * Best-effort deep link into the matching pane of System Settings. The
   * Tauri command may not be registered yet — callers must `await` inside a
   * try/catch and fall back to inline instructions on failure.
   */
  openSystemPrivacySettings: (pane?: string): Promise<{ opened: boolean }> =>
    invokeTauri<{ opened: boolean }>("open_system_privacy_settings", { pane }),
  restartApp: (): Promise<void> => invokeTauri<void>("restart_app"),
  startRecording: (options: {
    // The native bridge requires a concrete source identifier (for example
    // `display:<CGDirectDisplayID>` on macOS), never a synthetic/null value.
    sourceId: string;
    cameraId?: string | null;
    micId?: string | null;
    captureSystemAudio: boolean;
    fps: number;
    resolution: string;
    layout?: EditLayout;
    projectName?: string;
    projectDir?: string;
  }) =>
    invokeTauri<{
      sessionId: string;
      state: SessionState;
      startedAtUs: number;
      projectPath?: string;
    }>("start_recording", { options }),
  pauseRecording: () => invokeTauri<{ state: SessionState }>("pause_recording"),
  resumeRecording: () => invokeTauri<{ state: SessionState }>("resume_recording"),
  openProject: (path: string) => invokeTauri<OpenedProject>("open_project", { path }),
  closeProject: (projectHandle: string) => invokeTauri<void>("close_project", { projectHandle }),
  projectSegments: (projectHandle: string, trackId: string, offset = 0, limit = 100) =>
    invokeTauri<SegmentPage>("project_segments", { projectHandle, trackId, offset, limit }),
  projectWaveform: (
    projectHandle: string,
    trackId: string,
    startUs: number,
    endUs: number,
    bucketCount = 256,
  ) =>
    invokeTauri<WaveformPage>("project_waveform", {
      projectHandle,
      trackId,
      startUs,
      endUs,
      bucketCount,
    }),
  projectZoomSuggestions: (projectHandle: string, config?: ZoomConfig) =>
    invokeTauri<ZoomGeneration>(
      "project_zoom_suggestions",
      config ? { projectHandle, config } : { projectHandle },
    ),
  projectZoomAccept: (projectHandle: string, expectedRevision: number, ids: string[]) =>
    invokeTauri<OpenedProject>("project_zoom_accept", {
      projectHandle,
      expectedRevision,
      ids,
    }),
  projectZoomDismiss: (projectHandle: string, expectedRevision: number, ids: string[]) =>
    invokeTauri<OpenedProject>("project_zoom_dismiss", {
      projectHandle,
      expectedRevision,
      ids,
    }),
  projectZoomUpdate: (projectHandle: string, expectedRevision: number, zoom: ProjectZoom) =>
    invokeTauri<OpenedProject>("project_zoom_update", {
      projectHandle,
      expectedRevision,
      zoom,
    }),
  projectZoomAdd: (projectHandle: string, expectedRevision: number, input: ManualZoomInput) =>
    invokeTauri<OpenedProject>("project_zoom_add", {
      projectHandle,
      expectedRevision,
      input,
    }),
  projectZoomDelete: (projectHandle: string, expectedRevision: number, id: string) =>
    invokeTauri<OpenedProject>("project_zoom_delete", {
      projectHandle,
      expectedRevision,
      id,
    }),
  projectLayoutUpdate: (
    projectHandle: string,
    expectedRevision: number,
    layout: EditLayout,
    wallpaperSource?: string,
  ) =>
    invokeTauri<OpenedProject>(
      "project_layout_update",
      wallpaperSource
        ? { projectHandle, expectedRevision, layout, wallpaperSource }
        : { projectHandle, expectedRevision, layout },
    ),
  projectRippleCuts: (
    projectHandle: string,
    expectedRevision: number,
    cuts: EditCut[],
  ) =>
    invokeTauri<OpenedProject>("project_ripple_cuts", {
      projectHandle,
      expectedRevision,
      cuts,
    }),
  projectUndo: (projectHandle: string, expectedRevision: number) =>
    invokeTauri<OpenedProject>("project_undo", { projectHandle, expectedRevision }),
  projectRedo: (projectHandle: string, expectedRevision: number) =>
    invokeTauri<OpenedProject>("project_redo", { projectHandle, expectedRevision }),
  projectRename: (projectHandle: string, newName: string) =>
    invokeTauri<OpenedProject>("project_rename", { projectHandle, newName }),
  playbackStatus: (projectHandle: string) =>
    invokeTauri<PlaybackStatus>("playback_status", { projectHandle }),
  playbackPlay: (projectHandle: string) =>
    invokeTauri<PlaybackStatus>("playback_play", { projectHandle }),
  playbackPause: (projectHandle: string) =>
    invokeTauri<PlaybackStatus>("playback_pause", { projectHandle }),
  playbackSeek: (projectHandle: string, editedUs: number) =>
    invokeTauri<PlaybackStatus>("playback_seek", { projectHandle, editedUs: Math.max(0, Math.round(editedUs)) }),
  previewAttach: (windowLabel: string, hitMode: PreviewHitMode = "consume") =>
    invokeTauri<PreviewStatus>("preview_attach", { windowLabel, hitMode }),
  previewLayout: (viewport: PreviewViewport) =>
    invokeTauri<PreviewStatus>("preview_layout", { viewport }),
  previewPresentFixed: (r: number, g: number, b: number, generation = 0) =>
    invokeTauri<PreviewStatus>("preview_present_fixed", { r, g, b, generation }),
  previewPresentFixture: (path: string, generation = 0) =>
    invokeTauri<PreviewStatus>("preview_present_fixture", { path, generation }),
  capturePreviewConfigure: (enabled: boolean, sourceId?: string, cameraId?: string) =>
    invokeTauri<void>("capture_preview_configure", { enabled, sourceId: sourceId ?? null, cameraId: cameraId ?? null }),
  previewStatus: () => invokeTauri<PreviewStatus>("preview_status"),
  previewHitTest: (x: number, y: number) => invokeTauri<boolean>("preview_hit_test", { x, y }),
  previewDetach: (windowLabel: string, generation?: number) =>
    invokeTauri<PreviewStatus>("preview_detach", { windowLabel, generation }),
  windowIdentity: () => invokeTauri<WindowIdentity>("window_identity"),
  hudSnapshot: () => invokeTauri<HudSnapshot>("hud_snapshot"),
  hudUpdate: (expectedRevision: number, patch: HudSettingsPatch) =>
    invokeTauri<HudSnapshot>("hud_update", { expectedRevision, patch }),
  hudReconcileCameras: (cameras: HudCameraInfo[], selectedCameraId?: string | null) =>
    invokeTauri<HudSnapshot>("hud_reconcile_cameras", { cameras, selectedCameraId }),
  hudPreviewAttach: (windowLabel: string, hitMode: PreviewHitMode = "circle_pass_through") =>
    invokeTauri<HudSnapshot>("hud_preview_attach", { windowLabel, hitMode }),
  hudPreviewLayout: (viewport: PreviewViewport) =>
    invokeTauri<PreviewStatus>("hud_preview_layout", { viewport }),
  hudPreviewStatus: () => invokeTauri<PreviewStatus>("hud_preview_status"),
  hudClose: () => invokeTauri<HudSnapshot>("hud_close"),
  hudSetVisible: (visible: boolean) => invokeTauri<HudSnapshot>("hud_set_visible", { visible }),
  mediaInteropStatus: () => invokeTauri<MediaInteropStatus>("media_interop_status"),
  mediaRunParity: () => invokeTauri<MediaParityReport>("media_run_parity"),
  exportStart: (projectHandle: string, settings: ExportSettings) =>
    invokeTauri<ExportStatus>("export_start", { projectHandle, settings }),
  exportStatus: (jobId?: string) =>
    invokeTauri<ExportStatus>("export_status", { jobId: jobId ?? null }),
  exportCancel: (jobId: string) => invokeTauri<ExportStatus>("export_cancel", { jobId }),
  stopRecording: () => invokeTauri<StopRecordingResult>("stop_recording"),
  getSessionStatus: () =>
    invokeTauri<{
      state: SessionState;
      elapsedUs: number;
      droppedFrames: number;
      audioBufferUnderflows: number;
      gapsTotal: number;
      timestampRecordsDropped: number;
      lastRuntimeError?: { message: string };
      projectPath?: string;
    }>("get_session_status"),
  detectSilence: (projectHandle: string, trackId: string, config: SilenceConfig) =>
    invokeTauri<SilenceDetectionResult>("detect_silence", { projectHandle, trackId, config }),
  applyJumpCuts: (silenceBlockIds: string[]) =>
    invokeTauri<{ affectedIntervalsCount: number }>("apply_jump_cuts", { silenceBlockIds }),
  saveProject: (projectData: unknown) => invokeTauri<{ success: boolean }>("save_project", { projectData }),
  getDefaultProjectsDir: () => invokeTauri<string>("get_default_projects_dir"),
  pickProjectFolder: () => invokeTauri<string | null>("pick_project_folder"),
  pickSaveDirectory: () => invokeTauri<string | null>("pick_save_directory"),
  pickExportDestination: (projectHandle?: string) =>
    invokeTauri<string | null>("pick_export_destination", { projectHandle: projectHandle ?? null }),
  pickWallpaperSource: () => invokeTauri<string | null>("pick_wallpaper_source"),
  showInFinder: (path: string): Promise<void> =>
    invokeTauri<void>("show_in_finder", { path }),
  setWindowTitle: async (title: string): Promise<void> => {
    if (typeof document !== "undefined") {
      document.title = title;
    }
    if (isTauriEnvironment()) {
      try {
        await invokeTauri<void>("set_window_title", { title });
      } catch {
        try {
          const { getCurrentWindow } = await import("@tauri-apps/api/window");
          await getCurrentWindow().setTitle(title);
        } catch (err) {
          console.warn("[Tauri] Failed to set window title:", err);
        }
      }
    }
  },
};
