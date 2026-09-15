import React, { useEffect, useState, useCallback } from "react";
import { RecordScene } from "./components/scenes/RecordScene";
import { RecordingCompletedModal } from "./components/recording-hud/RecordingCompletedModal";
import { SceneErrorBoundary } from "./components/SceneErrorBoundary";
import { useSettingsStore } from "./stores/settingsStore";
import { useWindowTitle } from "./hooks/useWindowTitle";
import { useRecording } from "./hooks/useRecording";
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

let didAutoRequestScreen = false;

export const App: React.FC = () => {
  useWindowTitle();
  const recording = useRecording();
  const { reconcileSelections } = useSettingsStore();

  const [sources, setSources] = useState<CaptureSource[]>([]);
  const [cameras, setCameras] = useState<CameraDevice[]>([]);
  const [mics, setMics] = useState<AudioDevice[]>([]);
  const [permissions, setPermissions] = useState<PermissionBundle>(ZERO_PERMISSIONS);
  const [enumerationError, setEnumerationError] = useState<string>();

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

      const failures: string[] = [];
      if (sourcesRes.status === "rejected") failures.push(String(sourcesRes.reason));
      if (devicesRes.status === "rejected") failures.push(String(devicesRes.reason));
      setEnumerationError(failures.length > 0 ? failures.join(" · ") : undefined);

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

  useEffect(() => {
    const onDeviceOrFocusChange = () => {
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

  return (
    <div data-ui-root="studio" className="flex flex-col h-screen w-screen overflow-hidden bg-studio-950 text-studio-100 select-none">
      {/* Main Recording Scene Workspace */}
      <main className="flex-1 flex overflow-hidden relative">
        <SceneErrorBoundary>
          <RecordScene
            sources={sources}
            cameras={cameras}
            mics={mics}
            permissions={permissions}
            enumerationError={enumerationError}
            refreshPermissions={refreshPermissions}
            recording={recording}
            previewHidden={Boolean(recording.lastRecordingResult)}
          />
        </SceneErrorBoundary>
      </main>

      {/* 3. Recording Complete Modal */}
      {recording.lastRecordingResult && (
        <RecordingCompletedModal
          result={recording.lastRecordingResult}
          onDismiss={recording.dismissCompletedModal}
        />
      )}
    </div>
  );
};
