import React, { useEffect, useState, useCallback } from "react";
import { TopNavBar } from "./components/navigation/TopNavBar";
import { RecordScene } from "./components/scenes/RecordScene";
import { EditStudioScene } from "./components/scenes/EditStudioScene";
import { SilenceModal } from "./components/silence-modal/SilenceModal";
import { useSettingsStore } from "./stores/settingsStore";
import { api } from "./lib/ipc";
import {
  CaptureSource,
  CameraDevice,
  AudioDevice,
  PermissionBundle,
} from "./lib/types";

const ZERO_PERMISSIONS: PermissionBundle = {
  screenRecording: "unknown",
  camera: "unknown",
  microphone: "unknown",
};

// React StrictMode mounts twice in development. A process-level guard keeps the
// one-shot Screen Recording prompt from firing twice in a packaged build too.
let didAutoRequestScreen = false;

export const App: React.FC = () => {
  const { activeScene, reconcileSelections } = useSettingsStore();

  const [sources, setSources] = useState<CaptureSource[]>([]);
  const [cameras, setCameras] = useState<CameraDevice[]>([]);
  const [mics, setMics] = useState<AudioDevice[]>([]);
  const [permissions, setPermissions] = useState<PermissionBundle>(ZERO_PERMISSIONS);

  const refreshPermissions = useCallback(
    async (
      reRequest: boolean,
      which?: { screen?: boolean; camera?: boolean; microphone?: boolean },
    ): Promise<PermissionBundle> => {
      try {
        const next = reRequest
          ? await api.requestCapturePermissions({
              screen: which?.screen ?? true,
              camera: which?.camera ?? false,
              microphone: which?.microphone ?? false,
            })
          : await api.getPermissionStatus();
        setPermissions(next);
        return next;
      } catch (err) {
        console.warn("[Permissions] refresh failed:", err);
        return ZERO_PERMISSIONS;
      }
    },
    [],
  );

  const loadDevicesAndSources = useCallback(async () => {
    try {
      const [sourcesRes, devicesRes] = await Promise.allSettled([
        api.listCaptureSources(),
        api.listDevices(),
      ]);

      const loadedSources = sourcesRes.status === "fulfilled" ? sourcesRes.value : [];
      const loadedDevices =
        devicesRes.status === "fulfilled"
          ? devicesRes.value
          : { cameras: [], mics: [] };

      setSources(loadedSources);
      setCameras(loadedDevices.cameras);
      setMics(loadedDevices.mics);

      reconcileSelections(
        loadedSources,
        loadedDevices.cameras,
        loadedDevices.mics,
      );
    } catch (err) {
      console.error("[App] Error loading sources and devices:", err);
    }
  }, [reconcileSelections]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const current = await refreshPermissions(false);
      if (cancelled) return;
      // Prompt at most once per launch, and only for Screen Recording. Camera
      // and microphone dialogs must not fire on startup — that is what made
      // the packaged app unusable after the user had already granted access.
      if (current.screenRecording === "notDetermined" && !didAutoRequestScreen) {
        didAutoRequestScreen = true;
        await refreshPermissions(true, { screen: true, camera: false, microphone: false });
      }
      if (cancelled) return;
      await loadDevicesAndSources();
    })();
    return () => {
      cancelled = true;
    };
  }, [loadDevicesAndSources, refreshPermissions]);

  // Refresh devices and permissions when window regains focus or hardware changes
  useEffect(() => {
    const onDeviceOrFocusChange = () => {
      // Read-only. Requesting here re-opens the macOS permission dialog every
      // time the window refocuses after the user grants access in Settings.
      void refreshPermissions(false);
      void loadDevicesAndSources();
    };
    window.addEventListener("focus", onDeviceOrFocusChange);
    document.addEventListener("visibilitychange", onDeviceOrFocusChange);
    if (typeof navigator !== "undefined" && navigator.mediaDevices?.addEventListener) {
      navigator.mediaDevices.addEventListener("devicechange", onDeviceOrFocusChange);
    }
    return () => {
      window.removeEventListener("focus", onDeviceOrFocusChange);
      document.removeEventListener("visibilitychange", onDeviceOrFocusChange);
      if (typeof navigator !== "undefined" && navigator.mediaDevices?.removeEventListener) {
        navigator.mediaDevices.removeEventListener("devicechange", onDeviceOrFocusChange);
      }
    };
  }, [loadDevicesAndSources, refreshPermissions]);


  const handleOpenPrivacySettings = () => {
    void api.openSystemPrivacySettings("ScreenCapture");
  };

  return (
    <div className="flex flex-col h-screen w-screen overflow-hidden bg-studio-950 text-studio-100 select-none">
      {/* 1. Global Top Bar with Scene Switcher Card */}
      <TopNavBar
        permissions={permissions}
        onOpenSettings={handleOpenPrivacySettings}
      />

      {/* 2. Main Scene Workspace: Record Scene vs Edit Studio Scene */}
      <main className="flex-1 flex overflow-hidden relative">
        {activeScene === "record" ? (
          <RecordScene
            sources={sources}
            cameras={cameras}
            mics={mics}
            permissions={permissions}
            refreshPermissions={refreshPermissions}
          />
        ) : (
          <EditStudioScene />
        )}
      </main>

      {/* 3. Global Silence Detection Modal */}
      <SilenceModal />
    </div>
  );
};
