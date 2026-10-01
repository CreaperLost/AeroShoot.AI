import { useShallow } from "zustand/react/shallow";
import { selectCameraQuality, selectedCameraFormats, useSettingsStore } from "../../stores/settingsStore";
import {
  BITRATES_MBPS,
  CAMERA_BITRATES_MBPS,
  cameraMaximum,
  cameraOptions,
  CameraResolution,
  FRAME_RATES,
  megabytesPerMinute,
  Resolution,
  RESOLUTIONS,
} from "../../lib/recordingQuality";
import { SelectField } from "../ui/controls";

const fpsLabel = (fps: number) => `${fps} fps`;
const mbpsLabel = (mbps: number) => `${mbps} Mbps`;

/** Resolution, frame rate, and bitrate for the screen track. */
export function ScreenVideoSettings({ disabled }: { disabled: boolean }) {
  const { fps, setFps, resolution, setResolution, videoBitrateMbps, setVideoBitrateMbps } = useSettingsStore();
  return (
    <div>
      <div className="grid grid-cols-3 gap-2.5">
        <SelectField<Resolution>
          label="Resolution"
          value={resolution}
          options={RESOLUTIONS}
          format={(value) => value}
          onChange={setResolution}
          disabled={disabled}
        />
        <SelectField<number>
          label="Frame rate"
          value={fps}
          options={FRAME_RATES}
          format={fpsLabel}
          onChange={setFps}
          disabled={disabled}
        />
        <SelectField<number>
          label="Bitrate"
          value={videoBitrateMbps}
          options={BITRATES_MBPS}
          format={mbpsLabel}
          onChange={setVideoBitrateMbps}
          disabled={disabled}
        />
      </div>
      <p className="mt-1.5 text-xs text-studio-500">About {megabytesPerMinute(videoBitrateMbps)} MB per minute</p>
    </div>
  );
}

/** The camera's own track settings; shown only while a camera is recorded. */
export function CameraVideoSettings({ disabled, cameraOn }: { disabled: boolean; cameraOn: boolean }) {
  const { setCameraResolution, setCameraFps, cameraBitrateMbps, setCameraBitrateMbps } = useSettingsStore();
  const formats = useSettingsStore(selectedCameraFormats);
  // Shown lowered to what this camera delivers; the stored choice is kept
  // for cameras that can do more.
  const { resolution: cameraResolution, fps: cameraFps } = useSettingsStore(useShallow(selectCameraQuality));
  if (!cameraOn) {
    return <p className="text-xs text-studio-400">The camera is off. Turn it on under What to record.</p>;
  }
  const { resolutions, frameRates } = cameraOptions(formats, cameraResolution);
  const maximum = cameraMaximum(formats);
  return (
    <div>
      <div className="grid grid-cols-3 gap-2.5">
        <SelectField<CameraResolution>
          label="Resolution"
          value={cameraResolution}
          options={resolutions}
          format={(value) => value}
          onChange={setCameraResolution}
          disabled={disabled}
        />
        <SelectField<number>
          label="Frame rate"
          value={cameraFps}
          options={frameRates}
          format={fpsLabel}
          onChange={setCameraFps}
          disabled={disabled}
        />
        <SelectField<number>
          label="Bitrate"
          value={cameraBitrateMbps}
          options={CAMERA_BITRATES_MBPS}
          format={mbpsLabel}
          onChange={setCameraBitrateMbps}
          disabled={disabled}
        />
      </div>
      <p className="mt-1.5 text-xs text-studio-500">
        About {megabytesPerMinute(cameraBitrateMbps)} MB per minute
        {maximum && ` · This camera records up to ${maximum}`}
      </p>
    </div>
  );
}
