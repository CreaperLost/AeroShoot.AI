import { useEffect, useState } from "react";
import { NativePreviewHost } from "./NativePreviewHost";
import { api, isTauriEnvironment } from "../../lib/ipc";

// Serialize lifecycle requests, including StrictMode cleanup and device changes.
let configuration: Promise<unknown> = Promise.resolve();
function configure(enabled: boolean, sourceId?: string, cameraId?: string, captureScreen = true, captureSystemAudio = false, micId?: string, micGainDb = 0) {
  const next = configuration.catch(() => undefined).then(() => api.capturePreviewConfigure(enabled, sourceId, cameraId, captureScreen, captureSystemAudio, micId, micGainDb));
  configuration = next;
  return next;
}
export function CapturePreview({ sourceId, cameraId, micId, micGainDb = 0, captureScreen = true, captureSystemAudio = false, enabled, surfaceVisible = true }: { sourceId?: string; cameraId?: string; micId?: string; micGainDb?: number; captureScreen?: boolean; captureSystemAudio?: boolean; enabled: boolean; surfaceVisible?: boolean }) {
  const [error, setError] = useState<string>();
  useEffect(() => {
    if (!isTauriEnvironment()) return;
    let active = true;
    setError(undefined);
    void configure(enabled && Boolean(sourceId) && (captureScreen || captureSystemAudio || Boolean(cameraId) || Boolean(micId)), sourceId, cameraId, captureScreen, captureSystemAudio, micId, micGainDb).catch(err => {
      if (active) setError(String(err));
    });
    return () => { active = false; void configure(false).catch(() => undefined); };
  }, [sourceId, cameraId, micId, micGainDb, captureScreen, captureSystemAudio, enabled]);
  if (!isTauriEnvironment()) return <p>Open the AeroShoot desktop app to preview and record devices.</p>;
  return <div className="w-full h-full min-h-0 flex flex-col items-center gap-2">
    <NativePreviewHost live surfaceVisible={surfaceVisible} />
    {error && <p role="alert" className="text-sm text-rose-300">{error}</p>}
    {!enabled && <p className="text-xs text-studio-400">Preview waits for Screen Recording permission.</p>}
  </div>;
}
