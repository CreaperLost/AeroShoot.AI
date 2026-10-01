import { useShallow } from "zustand/react/shallow";
import { useEffect, useRef } from "react";
import { Camera, Folder, Monitor, Timer, X } from "lucide-react";
import { selectCameraQuality, useSettingsStore } from "../../stores/settingsStore";
import { CountdownControl } from "./CountdownControl";
import { MouseTelemetryControl } from "./MouseTelemetryControl";
import { ProjectDestinationBar } from "./ProjectDestinationBar";
import { CameraVideoSettings, ScreenVideoSettings } from "./RecordingQualityControl";
import { SettingsSection } from "../ui/controls";

/** "Screen 1080p60 · Camera 720p30" — the summary on the Settings button. */
export function useSettingsSummary(cameraOn: boolean): string {
  const { resolution, fps } = useSettingsStore();
  const camera = useSettingsStore(useShallow(selectCameraQuality));
  const parts = [`Screen ${resolution}${fps}`];
  if (cameraOn) parts.push(`Camera ${camera.resolution}${camera.fps}`);
  return parts.join(" · ");
}

/**
 * Recording settings that rarely change between takes. Changes apply
 * immediately; Done only closes. The native preview draws above web content,
 * so the caller hides it while this is open.
 */
export function RecordingSettingsDialog({
  open,
  onClose,
  cameraOn,
  disabled,
}: {
  open: boolean;
  onClose: () => void;
  cameraOn: boolean;
  disabled: boolean;
}) {
  const panelRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    panelRef.current?.querySelector<HTMLElement>("select, input, button")?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      previous?.focus();
    };
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="recording-settings-title"
        className="flex max-h-[90vh] w-full max-w-lg flex-col rounded-xl border border-studio-700 bg-studio-900 shadow-2xl"
      >
        <div className="flex items-center gap-2 border-b border-studio-800 px-5 py-3.5">
          <h2 id="recording-settings-title" className="flex-1 text-base font-medium text-white">
            Recording settings
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close settings"
            className="rounded-md p-1 text-studio-400 hover:bg-studio-800 hover:text-white"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4">
          <SettingsSection title="Screen video" icon={<Monitor className="h-4 w-4" />}>
            <ScreenVideoSettings disabled={disabled} />
          </SettingsSection>
          <SettingsSection title="Camera video" icon={<Camera className="h-4 w-4" />}>
            <CameraVideoSettings disabled={disabled} cameraOn={cameraOn} />
          </SettingsSection>
          <SettingsSection title="Saving" icon={<Folder className="h-4 w-4" />}>
            <ProjectDestinationBar disabled={disabled} />
          </SettingsSection>
          <SettingsSection title="Recording" icon={<Timer className="h-4 w-4" />}>
            <CountdownControl disabled={disabled} />
            <MouseTelemetryControl disabled={disabled} />
          </SettingsSection>
        </div>

        <div className="flex justify-end border-t border-studio-800 px-5 py-3">
          <button
            type="button"
            onClick={onClose}
            className="rounded-lg border border-studio-700 bg-studio-850 px-4 py-1.5 text-[13px] font-medium text-white hover:bg-studio-800"
          >
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
