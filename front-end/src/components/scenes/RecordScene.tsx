import React, { useCallback, useState } from "react";
import { ChevronRight, Settings } from "lucide-react";
import { DeviceControlDeck } from "../recording-hud/DeviceControlDeck";
import { useDefaultProjectDir } from "../recording-hud/ProjectDestinationBar";
import { RecordingSettingsDialog, useSettingsSummary } from "../recording-hud/RecordingSettingsDialog";
import { CapturePreview } from "../canvas/CapturePreview";
import { RecordingFloatingDock } from "../recording-hud/RecordingFloatingDock";
import { CaptureHealthBar } from "../recording-hud/CaptureHealthBar";
import { RecordingController } from "../../hooks/useRecording";
import { useSettingsStore } from "../../stores/settingsStore";
import { CaptureSource, CameraDevice, AudioDevice, PermissionBundle } from "../../lib/types";
import { api } from "../../lib/ipc";

interface RecordSceneProps {
  sources: CaptureSource[];
  cameras: CameraDevice[];
  mics: AudioDevice[];
  permissions: PermissionBundle;
  enumerationError?: string;
  refreshPermissions: (
    reRequest: boolean,
    which?: { screen?: boolean; camera?: boolean; microphone?: boolean },
  ) => Promise<PermissionBundle>;
  recording: RecordingController;
  /** Hide the native preview while a modal covers the scene; it draws above all web content. */
  previewHidden?: boolean;
}


export const RecordScene: React.FC<RecordSceneProps> = ({
  sources,
  cameras,
  mics,
  permissions,
  enumerationError,
  refreshPermissions,
  recording,
  previewHidden = false,
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
    captureHealth,
    countdownEndsAt,
  } = recording;

  useDefaultProjectDir();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const closeSettings = useCallback(() => setSettingsOpen(false), []);

  const selectedSource = sources.find((s) => s.id === settings.selectedSourceId);
  const selectedCamera = cameras.find((c) => c.id === settings.selectedCameraId) ?? cameras[0];

  const isRecording = sessionState === "recording";
  const isPaused = sessionState === "paused";
  const isTransitioning = sessionState === "preparing" || sessionState === "stopping";

  const isScreenBlocked =
    permissions.screenRecording === "denied" || permissions.screenRecording === "restricted";
  const screenReady = permissions.screenRecording === "authorized";
  const needsScreenPermission = Boolean(selectedSource) || settings.captureSystemAudio;
  const cameraSelected = settings.cameraBubble.enabled && Boolean(settings.selectedCameraId);
  const cameraUsable = cameraSelected && permissions.camera !== "denied" && permissions.camera !== "restricted";
  const micUsable = Boolean(settings.selectedMicId) && permissions.microphone !== "denied" && permissions.microphone !== "restricted";
  const hasMedia = Boolean(selectedSource) || cameraUsable || micUsable || settings.captureSystemAudio;
  const canStart = !isTransitioning && hasMedia && (!needsScreenPermission || screenReady);

  const disabledReason = (() => {
    if (!hasMedia) return "Turn on at least one source with permission to record.";
    if (isTransitioning) return "Engine transitioning...";
    if (needsScreenPermission && isScreenBlocked) return "Screen Recording permission denied in System Settings.";
    if (needsScreenPermission && !screenReady) return "Checking Screen Recording permission…";
    return undefined;
  })();

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

  const settingsSummary = useSettingsSummary(cameraUsable);

  const previewEnabled =
    ((Boolean(selectedSource) && screenReady) ||
      (cameraSelected && permissions.camera === "authorized") ||
      (Boolean(settings.selectedMicId) && permissions.microphone === "authorized") ||
      (settings.captureSystemAudio && screenReady)) &&
    sessionState !== "stopping";

  return (
    <div className="record-layout flex-1 w-full h-full min-h-0 overflow-hidden">
      {/* Left: live preview. The native AppKit view mirrors this column and
          draws above web content, so nothing interactive may overlap it. */}
      <section aria-label="Preview" className="flex min-h-0 min-w-0 flex-col gap-2 bg-studio-950 px-4 pb-4 pt-3">
        <p className="shrink-0 truncate text-xs text-studio-400">
          {selectedSource
            ? `${selectedSource.name} · ${selectedSource.width}×${selectedSource.height}`
            : "No screen — recording camera and audio only"}
        </p>
        <div className="flex min-h-0 flex-1">
          <CapturePreview
            sourceId={selectedSource?.id ?? settings.availableSourceFallbackId ?? undefined}
            captureScreen={Boolean(selectedSource)}
            captureSystemAudio={settings.captureSystemAudio && screenReady}
            cameraId={settings.cameraBubble.enabled && permissions.camera === "authorized" ? selectedCamera?.id : undefined}
            micId={permissions.microphone === "authorized" ? settings.selectedMicId ?? undefined : undefined}
            micGainDb={settings.micGainDb}
            enabled={previewEnabled}
            surfaceVisible={!previewHidden && !settingsOpen}
          />
        </div>
      </section>

      {/* Right: options, with capture status and record controls pinned at the bottom. */}
      <aside aria-label="Recording options" className="record-sidebar flex min-h-0 min-w-0 flex-col bg-studio-900/70">
        <div className="flex-1 min-h-0 space-y-3 overflow-y-auto px-4 py-4">
          {enumerationError && (
            <p role="alert" className="rounded-lg border border-rose-800/60 bg-rose-950/60 px-3 py-2 text-xs text-rose-200">
              {enumerationError}
            </p>
          )}
          <h2 className="text-sm font-medium text-white">What to record</h2>
          <DeviceControlDeck
            sources={sources}
            cameras={cameras}
            mics={mics}
            disabled={isRecording || isPaused}
            permissions={permissions}
            needsScreenPermission={needsScreenPermission}
            onRequestScreenPermission={async () => {
              if (permissions.screenRecording === "denied" || permissions.screenRecording === "restricted") {
                await api.openSystemPrivacySettings("ScreenCapture");
                return;
              }
              await refreshPermissions(true, { screen: true, camera: false, microphone: false });
            }}
            onRequestCameraPermission={async () => {
              if (permissions.camera === "denied" || permissions.camera === "restricted") {
                await api.openSystemPrivacySettings("Camera");
                return;
              }
              if (permissions.camera !== "authorized") {
                await refreshPermissions(true, { screen: false, camera: true, microphone: false });
              }
            }}
            onRequestMicrophonePermission={async () => {
              if (permissions.microphone === "denied" || permissions.microphone === "restricted") {
                await api.openSystemPrivacySettings("Microphone");
                return;
              }
              if (permissions.microphone !== "authorized") {
                await refreshPermissions(true, { screen: false, camera: false, microphone: true });
              }
            }}
          />
          <button
            type="button"
            onClick={() => setSettingsOpen(true)}
            disabled={isRecording || isPaused || isTransitioning}
            title={isRecording || isPaused ? "Settings can't change during a recording" : undefined}
            className="flex w-full items-center gap-3 rounded-lg border border-studio-700 bg-studio-850 px-3 py-2.5 text-left hover:bg-studio-800 disabled:cursor-not-allowed disabled:opacity-50"
          >
            <Settings aria-hidden="true" className="h-4 w-4 text-studio-300" />
            <span className="text-[13px] font-medium text-white">Settings</span>
            <span className="min-w-0 flex-1 truncate text-right text-xs text-studio-400">{settingsSummary}</span>
            <ChevronRight aria-hidden="true" className="h-3.5 w-3.5 text-studio-500" />
          </button>
        </div>

        <div className="shrink-0 space-y-2.5 border-t border-studio-800/80 bg-studio-900/95 px-4 py-3">
          {(isRecording || isPaused || (sessionState === "error" && sessionOwned)) && (
            <CaptureHealthBar
              health={captureHealth}
              screenEnabled={Boolean(selectedSource)}
              cameraEnabled={settings.cameraBubble.enabled && permissions.camera === "authorized"}
              systemAudioEnabled={settings.captureSystemAudio}
              micEnabled={Boolean(settings.selectedMicId) && permissions.microphone === "authorized"}
              paused={isPaused}
            />
          )}
          {error && <p role="alert" className="text-xs text-rose-300">{error}</p>}
          <RecordingFloatingDock
            sessionState={sessionState}
            elapsedMs={elapsedMs}
            countdownEndsAt={countdownEndsAt}
            canStart={canStart}
            sessionOwned={sessionOwned}
            disabledReason={disabledReason}
            onStart={handleStart}
            onPause={pauseRecording}
            onResume={resumeRecording}
            onStop={() => void stopRecording()}
          />
        </div>
      </aside>

      <RecordingSettingsDialog
        open={settingsOpen}
        onClose={closeSettings}
        cameraOn={settings.cameraBubble.enabled}
        disabled={isRecording || isPaused || isTransitioning}
      />
    </div>
  );
};
