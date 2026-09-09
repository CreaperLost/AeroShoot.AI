import { useState, useEffect, useCallback, useRef } from "react";
import { SessionState } from "../lib/types";
import { api } from "../lib/ipc";
import { useSettingsStore } from "../stores/settingsStore";
import { useProjectStore } from "../stores/projectStore";

export function useRecording() {
  const [sessionState, setSessionState] = useState<SessionState>("idle");
  const [error, setError] = useState<string>();
  const [elapsedMs, setElapsedMs] = useState(0);
  const [droppedFrames] = useState(0);

  const settings = useSettingsStore();
  const project = useProjectStore();

  const timerRef = useRef<number | null>(null);

  // Poll or tick elapsed time when recording
  useEffect(() => {
    if (sessionState === "recording") {
      const startTime = Date.now() - elapsedMs;
      timerRef.current = window.setInterval(() => {
        setElapsedMs(Date.now() - startTime);
      }, 50);
    } else {
      if (timerRef.current) {
        clearInterval(timerRef.current);
        timerRef.current = null;
      }
    }

    return () => {
      if (timerRef.current) {
        clearInterval(timerRef.current);
      }
    };
  }, [sessionState]);

  const startRecording = useCallback(async () => {
    // The Rust/Swift bridge requires a concrete native source identifier.
    // Keep this guard here as well as in the HUD so callers cannot start a
    // session during the initial enumeration window.
    const sourceId = settings.selectedSourceId;
    if (!settings.selectionsReady || !sourceId) {
      console.warn("Cannot start recording before capture source enumeration is ready");
      return;
    }

    try {
      setError(undefined);
      setSessionState("preparing");
      const res = await api.startRecording({
        sourceId,
        cameraId: settings.cameraBubble.enabled ? settings.selectedCameraId : undefined,
        micId: settings.selectedMicId,
        captureSystemAudio: settings.captureSystemAudio,
        fps: settings.fps,
        resolution: settings.resolution,
      });
      setSessionState(res.state);
      setElapsedMs(0);
    } catch (err) {
      console.error("Failed to start recording:", err);
      setError(String(err));
      setSessionState("error");
    }
  }, [settings]);

  const pauseRecording = useCallback(async () => {
    try {
      const res = await api.pauseRecording();
      setSessionState(res.state);
    } catch (err) {
      console.error("Failed to pause recording:", err);
    }
  }, []);

  const resumeRecording = useCallback(async () => {
    try {
      const res = await api.resumeRecording();
      setSessionState(res.state);
    } catch (err) {
      console.error("Failed to resume recording:", err);
    }
  }, []);

  const stopRecording = useCallback(async (): Promise<boolean> => {
    try {
      setError(undefined);
      setSessionState("stopping");
      const res = await api.stopRecording();
      setSessionState(res.state);
      if (!res.projectPath) {
        throw new Error("Stop did not return a project path. Open the recording in the desktop app.");
      }
      const previous = useProjectStore.getState().openedProject;
      if (previous) {
        try {
          await api.closeProject(previous.projectHandle);
        } catch {
          // The following open replaces the backend handle.
        }
      }
      const opened = await api.openProject(res.projectPath);
      project.loadOpenedProject(opened);
      return true;
    } catch (err) {
      console.error("Failed to stop recording or open its project:", err);
      setError(String(err));
      setSessionState("error");
      return false;
    }
  }, [project]);

  return {
    error,
    sessionState,
    elapsedMs,
    droppedFrames,
    startRecording,
    pauseRecording,
    resumeRecording,
    stopRecording,
  };
}
