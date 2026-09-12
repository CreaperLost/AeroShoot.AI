import React from "react";
import { MouseTelemetryControl } from "../recording-hud/MouseTelemetryControl";
import { DeviceControlDeck } from "../recording-hud/DeviceControlDeck";
import { ProjectDestinationBar } from "../recording-hud/ProjectDestinationBar";
import { CapturePreview } from "../canvas/CapturePreview";
import { RecordingFloatingDock } from "../recording-hud/RecordingFloatingDock";
import { InspectorPanel } from "../inspector/InspectorPanel";
import { RecordingController } from "../../hooks/useRecording";
import { useSettingsStore } from "../../stores/settingsStore";
import { CaptureSource, CameraDevice, AudioDevice, PermissionBundle } from "../../lib/types";
import { api } from "../../lib/ipc";

interface RecordSceneProps {
  sources: CaptureSource[];
  cameras: CameraDevice[];
  mics: AudioDevice[];
  permissions: PermissionBundle;
  refreshPermissions: (
    reRequest: boolean,
    which?: { screen?: boolean; camera?: boolean; microphone?: boolean },
  ) => Promise<PermissionBundle>;
  recording: RecordingController;
}

export const RecordScene: React.FC<RecordSceneProps> = ({
  sources,
  cameras,
  mics,
  permissions,
  refreshPermissions,
  recording,
}) => {
  const settings = useSettingsStore();
  const {
    error,
    sessionState,
    elapsedMs,
    sessionOwned,
    startRecording,
    pauseRecording,
    resumeRecording,
    stopRecording,
  } = recording;

  const selectedSource = sources.find((s) => s.id === settings.selectedSourceId) ?? sources[0];
  const selectedCamera = cameras.find((c) => c.id === settings.selectedCameraId) ?? cameras[0];

  const isRecording = sessionState === "recording";
  const isPaused = sessionState === "paused";
  const isTransitioning = sessionState === "preparing" || sessionState === "stopping";

  const isScreenBlocked =
    permissions.screenRecording === "denied" || permissions.screenRecording === "restricted";
  const screenReady = permissions.screenRecording === "authorized";
  const cameraDenied =
    settings.cameraBubble.enabled &&
    (permissions.camera === "denied" || permissions.camera === "restricted");
  const micDenied =
    Boolean(settings.selectedMicId) &&
    (permissions.microphone === "denied" || permissions.microphone === "restricted");

  const canStart =
    !isTransitioning &&
    Boolean(selectedSource) &&
    screenReady;

  const previewAspectRatio =
    settings.canvas.aspectRatio === "9:16"
      ? 9 / 16
      : settings.canvas.aspectRatio === "4:3"
        ? 4 / 3
        : settings.canvas.aspectRatio === "1:1"
          ? 1
          : 16 / 9;

  const disabledReason = (() => {
    if (!selectedSource) return "Pick a capture source to record.";
    if (isTransitioning) return "Engine transitioning...";
    if (isScreenBlocked) return "Screen Recording permission denied in System Settings.";
    if (!screenReady) return "Checking Screen Recording permission…";
    return undefined;
  })();

  const handleStop = async () => {
    await stopRecording();
  };

  const handleStart = async () => {
    const needsCameraPermission =
      settings.cameraBubble.enabled && permissions.camera === "notDetermined";
    const needsMicrophonePermission =
      Boolean(settings.selectedMicId) && permissions.microphone === "notDetermined";
    if (needsCameraPermission || needsMicrophonePermission) {
      // Optional device prompts belong to the explicit Record action, never app startup.
      await refreshPermissions(true, {
        screen: false,
        camera: needsCameraPermission,
        microphone: needsMicrophonePermission,
      });
    }
    await startRecording();
  };

  return (
    <div className="record-scene-grid flex-1 w-full h-full overflow-hidden relative">
      {error && <p role="alert" className="px-5 py-2 text-sm text-rose-300">{error}</p>}
      {/* 1. Device Selection Bar (First-class selectors) */}
      <DeviceControlDeck
        sources={sources}
        cameras={cameras}
        mics={mics}
        disabled={isRecording || isPaused}
      />
      <ProjectDestinationBar disabled={isRecording || isPaused || isTransitioning} />

      <MouseTelemetryControl disabled={isRecording || isPaused || isTransitioning} />

      {isScreenBlocked && (
        <div className="bg-amber-950/60 border-b border-amber-800/60 px-5 py-2.5 flex items-center justify-between text-xs text-amber-200">
          <div className="flex items-center space-x-2">
            <span className="font-semibold text-amber-300">Screen Recording:</span>
            <span>If you just allowed AeroShoot in System Settings, macOS requires quitting and reopening the app to take effect.</span>
          </div>
          <div className="flex items-center space-x-2 shrink-0">
            <button
              onClick={() => void api.openSystemPrivacySettings("ScreenCapture")}
              className="px-2.5 py-1 rounded bg-amber-900/50 hover:bg-amber-800/60 text-amber-200 text-xs font-medium border border-amber-700/50 transition-colors"
            >
              Open Settings
            </button>
            <button
              onClick={() => void api.restartApp()}
              className="px-2.5 py-1 rounded bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-medium transition-colors shadow-sm"
            >
              Restart App
            </button>
          </div>
        </div>
      )}
      {!screenReady && !isScreenBlocked && (
        <div className="bg-studio-900/80 border-b border-studio-800 px-5 py-2 flex items-center justify-between text-xs text-studio-300">
          <span>
            {permissions.screenRecording === "notDetermined"
              ? "Screen Recording permission is needed before capture can start."
              : "Checking Screen Recording permission…"}
          </span>
          <button
            type="button"
            onClick={() => void refreshPermissions(true, { screen: true, camera: false, microphone: false })}
            className="px-2.5 py-1 rounded bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-medium"
          >
            Allow Screen Recording
          </button>
        </div>
      )}
      {cameraDenied && (
        <div className="bg-studio-900/80 border-b border-studio-800 px-5 py-2 text-xs text-amber-300">
          Camera permission is off — recording will continue without the webcam bubble.
        </div>
      )}
      {micDenied && (
        <div className="bg-studio-900/80 border-b border-studio-800 px-5 py-2 text-xs text-amber-300">
          Microphone permission is off — recording will continue without mic audio.
        </div>
      )}

      {/* 2. Workspace: Canvas Preview + Customizer Panel */}
      <div className="record-workspace-grid min-h-0 overflow-hidden relative">
        {/* Center Canvas Stage with cleanly spaced Recording Dock */}
        <div className="flex-1 flex flex-col items-center justify-between relative overflow-hidden bg-studio-950 p-4">
          <div className="flex-1 w-full min-w-0 flex items-center justify-center min-h-0 overflow-hidden">
            <CapturePreview sourceId={selectedSource?.id} cameraId={settings.cameraBubble.enabled && permissions.camera === "authorized" ? selectedCamera?.id : undefined} enabled={screenReady && sessionState !== "stopping"} aspectRatio={previewAspectRatio} />
          </div>

          {/* Cleanly docked Recording Action Bar below the canvas */}
          <div className="pt-2 shrink-0 z-30">
            <RecordingFloatingDock
              sessionState={sessionState}
              elapsedMs={elapsedMs}
              canStart={canStart}
              sessionOwned={sessionOwned}
              disabledReason={disabledReason}
              onStart={handleStart}
              onPause={pauseRecording}
              onResume={resumeRecording}
              onStop={handleStop}
            />
          </div>
        </div>

        {/* Right Studio Customizer / Inspector */}
        <InspectorPanel />
      </div>
    </div>
  );
};
