import { useEffect, useState } from "react";
import { NativePreviewHost } from "./NativePreviewHost";
import { api, isTauriEnvironment } from "../../lib/ipc";

// Serialize lifecycle requests, including StrictMode cleanup and device changes.
let configuration: Promise<unknown> = Promise.resolve();
function configure(enabled: boolean, sourceId?: string, cameraId?: string) {
  const next = configuration.catch(() => undefined).then(() => api.capturePreviewConfigure(enabled, sourceId, cameraId));
  configuration = next;
  return next;
}
export function CapturePreview({ sourceId, cameraId, enabled, aspectRatio }: { sourceId?: string; cameraId?: string; enabled: boolean; aspectRatio: number }) {
  const [error, setError] = useState<string>();
  useEffect(() => {
    if (!isTauriEnvironment()) return;
    let active = true;
    setError(undefined);
    void configure(enabled && Boolean(sourceId), sourceId, cameraId).catch(err => {
      if (active) setError(String(err));
    });
    return () => { active = false; void configure(false).catch(() => undefined); };
  }, [sourceId, cameraId, enabled]);
  if (!isTauriEnvironment()) return <p>Open the AeroShoot desktop app to preview and record devices.</p>;
  return <div className="w-full h-full min-h-0 flex flex-col items-center gap-2">
    <NativePreviewHost live fitAspectRatio={aspectRatio} />
    {error && <p role="alert" className="text-sm text-rose-300">{error}</p>}
    {!enabled && <p className="text-xs text-studio-400">Preview waits for Screen Recording permission.</p>}
  </div>;
}
