import { useState, useEffect, useCallback } from "react";
import { layoutFromSettings, SessionState, StopRecordingResult } from "../lib/types";
import { api } from "../lib/ipc";
import { useSettingsStore } from "../stores/settingsStore";

export type RecordingController = ReturnType<typeof useRecording>;

export interface CaptureHealth {
  screenSamples: number;
  cameraSamples: number;
  systemAudioSamples: number;
  micSamples: number;
  systemAudioPeakDb?: number;
  micPeakDb?: number;
  droppedFrames: number;
  audioBufferUnderflows: number;
  screenLastSampleAgeMs?: number;
  cameraLastSampleAgeMs?: number;
  systemAudioLastSampleAgeMs?: number;
  micLastSampleAgeMs?: number;
  screenSegments: number;
  cameraSegments: number;
  systemAudioSegments: number;
  micSegments: number;
  firstTerminalError?: { trackId: string; errorCode: number; message: string };
}

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
  const [captureHealth, setCaptureHealth] = useState<CaptureHealth>({
    screenSamples: 0, cameraSamples: 0, systemAudioSamples: 0, micSamples: 0,
    droppedFrames: 0, audioBufferUnderflows: 0,
    screenSegments: 0, cameraSegments: 0, systemAudioSegments: 0, micSegments: 0,
  });
  const [sessionOwned, setSessionOwned] = useState(false);
  const [lastRecordingResult, setLastRecordingResult] = useState<StopRecordingResult | null>(null);
  /** When the start countdown reaches zero (ms since epoch), while starting. */
  const [countdownEndsAt, setCountdownEndsAt] = useState<number | null>(null);

  const settings = useSettingsStore();

  const applyStatus = useCallback(
    (status: {
      state: SessionState;
      elapsedUs: number;
      projectPath?: string;
      lastRuntimeError?: { trackId: string; errorCode: number; message: string; recoverable: boolean };
      firstTerminalError?: { trackId: string; errorCode: number; message: string; recoverable: boolean };
      screenSamples: number;
      cameraSamples: number;
      systemAudioSamples: number;
      micSamples: number;
      systemAudioPeakDb?: number;
      micPeakDb?: number;
      droppedFrames: number;
      audioBufferUnderflows: number;
      screenLastSampleAgeMs?: number;
      cameraLastSampleAgeMs?: number;
      systemAudioLastSampleAgeMs?: number;
      micLastSampleAgeMs?: number;
      screenSegments: number;
      cameraSegments: number;
      systemAudioSegments: number;
      micSegments: number;
    }) => {
      const next = status.state === "completed" ? "idle" : status.state;
      setSessionState(next);
      setElapsedMs(status.elapsedUs / 1000);
      setSessionOwned(sessionOwnsProject(status.state, status.projectPath));
      // Rust serializes absent Option fields as `null`; normalize to undefined
      // so components can rely on the declared `number | undefined` types.
      setCaptureHealth((previous) => ({
        screenSamples: status.screenSamples, cameraSamples: status.cameraSamples,
        systemAudioSamples: status.systemAudioSamples, micSamples: status.micSamples,
        systemAudioPeakDb: status.systemAudioPeakDb ?? undefined, micPeakDb: status.micPeakDb ?? undefined,
        droppedFrames: status.droppedFrames, audioBufferUnderflows: status.audioBufferUnderflows,
        screenLastSampleAgeMs: status.screenLastSampleAgeMs ?? undefined,
        cameraLastSampleAgeMs: status.cameraLastSampleAgeMs ?? undefined,
        systemAudioLastSampleAgeMs: status.systemAudioLastSampleAgeMs ?? undefined,
        micLastSampleAgeMs: status.micLastSampleAgeMs ?? undefined,
        screenSegments: status.screenSegments,
        cameraSegments: status.cameraSegments,
        systemAudioSegments: status.systemAudioSegments,
        micSegments: status.micSegments,
        firstTerminalError: previous.firstTerminalError ?? (
          status.firstTerminalError
            ? {
                trackId: status.firstTerminalError.trackId,
                errorCode: status.firstTerminalError.errorCode,
                message: status.firstTerminalError.message,
              }
            : undefined
        ),
      }));
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
    const sourceId = settings.selectedSourceId ?? settings.availableSourceFallbackId;
    const captureScreen = Boolean(settings.selectedSourceId);
    const hasMedia = captureScreen || (settings.cameraBubble.enabled && Boolean(settings.selectedCameraId)) || Boolean(settings.selectedMicId) || settings.captureSystemAudio;
    if (!settings.selectionsReady || !sourceId || !hasMedia) {
      console.warn("Cannot start recording before source selection is ready or with every source off");
      return;
    }

    try {
      setError(undefined);
      setCaptureHealth((health) => ({ ...health, firstTerminalError: undefined }));
      setLastRecordingResult(null);
      setSessionState("preparing");
      const startDelayMs = settings.countdownSeconds * 1000;
      setCountdownEndsAt(startDelayMs > 0 ? Date.now() + startDelayMs : null);
      const projectName = settings.projectName.trim();
      const res = await api.startRecording({
        sourceId,
        captureScreen,
        cameraId: settings.cameraBubble.enabled ? settings.selectedCameraId : undefined,
        micId: settings.selectedMicId,
        captureSystemAudio: settings.captureSystemAudio,
        fps: settings.fps,
        resolution: settings.resolution,
        layout: layoutFromSettings(settings.canvas, settings.cameraBubble),
        projectName: projectName || undefined,
        projectDir: settings.projectDir || undefined,
        micGainDb: settings.selectedMicId ? settings.micGainDb : undefined,
        videoBitrateBps: settings.videoBitrateMbps * 1_000_000,
        captureMouse: settings.captureMouse,
        startDelayMs,
      });
      setCountdownEndsAt(null);
      setSessionState(res.state);
      setSessionOwned(true);
      setElapsedMs(0);
      if (res.projectPath) {
        settings.setCreatedProjectPath(res.projectPath);
      }
    } catch (err) {
      console.error("Failed to start recording:", err);
      setCountdownEndsAt(null);
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
    droppedFrames: captureHealth.droppedFrames,
    captureHealth,
    sessionOwned,
    lastRecordingResult,
    countdownEndsAt,
    dismissCompletedModal,
    startRecording,
    pauseRecording,
    resumeRecording,
    stopRecording,
  };
}
