import { useState, useEffect, useCallback } from "react";
import { layoutFromSettings, SessionState, StopRecordingResult } from "../lib/types";
import { api } from "../lib/ipc";
import { useSettingsStore } from "../stores/settingsStore";

export type RecordingController = ReturnType<typeof useRecording>;

function sessionOwnsProject(state: SessionState, projectPath?: string): boolean {
  if (state === "recording" || state === "paused" || state === "preparing" || state === "stopping") {
    return true;
  }
  return state === "error" && Boolean(projectPath);
}

export function useRecording() {
  const [sessionState, setSessionState] = useState<SessionState>("idle");
  const [error, setError] = useState<string>();
  const [elapsedMs, setElapsedMs] = useState(0);
  const [droppedFrames] = useState(0);
  const [sessionOwned, setSessionOwned] = useState(false);
  const [lastRecordingResult, setLastRecordingResult] = useState<StopRecordingResult | null>(null);

  const settings = useSettingsStore();

  const applyStatus = useCallback(
    (status: {
      state: SessionState;
      elapsedUs: number;
      projectPath?: string;
      lastRuntimeError?: { message: string };
    }) => {
      const next = status.state === "completed" ? "idle" : status.state;
      setSessionState(next);
      setElapsedMs(status.elapsedUs / 1000);
      setSessionOwned(sessionOwnsProject(status.state, status.projectPath));
      if (status.lastRuntimeError) setError(status.lastRuntimeError.message);
    },
    [],
  );

  useEffect(() => {
    let active = true;
    void api
      .getSessionStatus()
      .then((status) => {
        if (!active) return;
        applyStatus(status);
      })
      .catch((err) => {
        if (active) setError(String(err));
      });
    return () => {
      active = false;
    };
  }, [applyStatus]);

  useEffect(() => {
    if (
      sessionState !== "recording" &&
      sessionState !== "paused" &&
      sessionState !== "preparing" &&
      sessionState !== "stopping" &&
      sessionState !== "error"
    ) {
      return;
    }
    let active = true;
    let pending = false;
    const timer = window.setInterval(() => {
      if (pending) return;
      pending = true;
      void api
        .getSessionStatus()
        .then((status) => {
          if (!active) return;
          applyStatus(status);
        })
        .catch((err) => {
          if (active) setError(String(err));
        })
        .finally(() => {
          pending = false;
        });
    }, 250);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [sessionState, applyStatus]);

  const startRecording = useCallback(async () => {
    const sourceId = settings.selectedSourceId;
    if (!settings.selectionsReady || !sourceId) {
      console.warn("Cannot start recording before capture source enumeration is ready");
      return;
    }

    try {
      setError(undefined);
      setLastRecordingResult(null);
      setSessionState("preparing");
      const projectName = settings.projectName.trim();
      const res = await api.startRecording({
        sourceId,
        cameraId: settings.cameraBubble.enabled ? settings.selectedCameraId : undefined,
        micId: settings.selectedMicId,
        captureSystemAudio: settings.captureSystemAudio,
        fps: settings.fps,
        resolution: settings.resolution,
        layout: layoutFromSettings(settings.canvas, settings.cameraBubble),
        projectName: projectName || undefined,
        projectDir: settings.projectDir || undefined,
        micGainDb: settings.selectedMicId ? settings.micGainDb : undefined,
      });
      setSessionState(res.state);
      setSessionOwned(true);
      setElapsedMs(0);
      if (res.projectPath) {
        settings.setCreatedProjectPath(res.projectPath);
      }
    } catch (err) {
      console.error("Failed to start recording:", err);
      setError(String(err));
      try {
        const status = await api.getSessionStatus();
        applyStatus(status);
        if (status.state === "idle" || status.state === "completed") {
          setSessionState("error");
        }
      } catch {
        setSessionState("error");
      }
    }
  }, [settings, applyStatus]);

  const pauseRecording = useCallback(async () => {
    try {
      const res = await api.pauseRecording();
      setSessionState(res.state);
      setError(undefined);
    } catch (err) {
      console.error("Failed to pause recording:", err);
      setError(String(err));
    }
  }, []);

  const resumeRecording = useCallback(async () => {
    try {
      const res = await api.resumeRecording();
      setSessionState(res.state);
      setError(undefined);
    } catch (err) {
      console.error("Failed to resume recording:", err);
      setError(String(err));
    }
  }, []);

  const stopRecording = useCallback(async (): Promise<boolean> => {
    try {
      setError(undefined);
      setSessionState("stopping");
      const res = await api.stopRecording();
      setSessionState(res.state === "completed" ? "idle" : res.state);
      setSessionOwned(false);
      if (res.projectPath) {
        settings.setCreatedProjectPath(res.projectPath);
      }
      setLastRecordingResult(res);
      return true;
    } catch (err) {
      console.error("Failed to stop recording:", err);
      setError(String(err));
      try {
        const status = await api.getSessionStatus();
        applyStatus(status);
        if (!sessionOwnsProject(status.state, status.projectPath)) {
          setSessionState("error");
        }
      } catch {
        setSessionState("error");
      }
      return false;
    }
  }, [settings, applyStatus]);

  const dismissCompletedModal = useCallback(() => {
    setLastRecordingResult(null);
  }, []);

  return {
    error,
    sessionState,
    elapsedMs,
    droppedFrames,
    sessionOwned,
    lastRecordingResult,
    dismissCompletedModal,
    startRecording,
    pauseRecording,
    resumeRecording,
    stopRecording,
  };
}
