import { useEffect, useRef, useState } from "react";
import { api } from "../../lib/ipc";
import { PreviewHitMode, PreviewStatus } from "../../lib/types";

interface NativePreviewHostProps {
  live?: boolean;
  windowLabel?: string;
  hitMode?: PreviewHitMode;
  className?: string;
  surface?: "studio" | "hud";
  fitAspectRatio?: number;
  showStatus?: boolean;
}

export function NativePreviewHost({
  live = true,
  windowLabel = "main",
  hitMode = "consume",
  className = "w-full max-w-4xl aspect-video",
  surface = "studio",
  fitAspectRatio,
  showStatus = true,
}: NativePreviewHostProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const [status, setStatus] = useState<PreviewStatus | null>(null);
  const [error, setError] = useState<string>();

  // The studio (`main` window) and HUD (`camera_overlay` window) surfaces are
  // owned by independent `PreviewOwner` instances on the Rust side and exposed
  // through separate Tauri command sets. Route attach / layout / status /
  // detach calls accordingly so the host works in both windows without
  // window-label-rejection errors.
  const isHud = windowLabel === "camera_overlay";

  useEffect(() => {
    let cancelled = false;
    let revision = 0;
    let generation: number | undefined;
    let animation = 0;
    let lastGeometry = "";
    let sending = false;
    const update = () => {
      if (cancelled) return;
      animation = requestAnimationFrame(update);
      const el = hostRef.current;
      if (!el || generation === undefined || sending) return;
      const rect = el.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return;
      let left = Math.max(0, rect.left), top = Math.max(0, rect.top);
      let right = Math.min(window.innerWidth, rect.right), bottom = Math.min(window.innerHeight, rect.bottom);
      let ancestor = el.parentElement;
      while (ancestor) {
        const style = getComputedStyle(ancestor);
        const bounds = ancestor.getBoundingClientRect();
        if (/(auto|scroll|hidden|clip)/.test(style.overflowX)) { left = Math.max(left, bounds.left); right = Math.min(right, bounds.right); }
        if (/(auto|scroll|hidden|clip)/.test(style.overflowY)) { top = Math.max(top, bounds.top); bottom = Math.min(bottom, bounds.bottom); }
        ancestor = ancestor.parentElement;
      }
      const clip: [number, number, number, number] = [left - rect.left, top - rect.top, Math.max(0, right - left), Math.max(0, bottom - top)];
      const visible = !document.hidden && clip[2] > 0 && clip[3] > 0;
      const viewport = { windowLabel, x: rect.left, y: rect.top, width: rect.width, height: rect.height,
        backingScale: window.devicePixelRatio || 1, visible, occluded: !visible, generation, clip };
      const key = JSON.stringify(viewport);
      if (key === lastGeometry) return;
      sending = true;
      const layout = isHud ? api.hudPreviewLayout : api.studioPreviewLayout;
      void layout({ ...viewport, revision: ++revision })
        .then(next => { if (!cancelled) { lastGeometry = key; setStatus(next); setError(undefined); } })
        .catch(err => { if (!cancelled) setError(String(err)); })
        .finally(() => { sending = false; });
    };
    const attach = isHud
      ? api.hudPreviewAttach(windowLabel, hitMode).then(() => api.hudPreviewStatus())
      : api.studioPreviewAttach(windowLabel, hitMode).then(() => api.studioPreviewStatus());
    void attach.then(attached => {
      generation = attached.generation;
      if (cancelled) {
        void (isHud ? api.hudClose() : api.studioPreviewDetach()).catch(() => undefined);
        return;
      }
      setStatus(attached); setError(undefined);
      update();
    }).catch(err => { if (!cancelled) setError(String(err)); });
    return () => {
      cancelled = true;
      cancelAnimationFrame(animation);
      if (generation !== undefined) {
        void (isHud ? api.hudClose() : api.studioPreviewDetach()).catch(() => undefined);
      }
    };
  }, [windowLabel, hitMode, surface, isHud]);

  return (
    <div className={`w-full flex flex-col items-center gap-2 ${fitAspectRatio || surface === "hud" ? "h-full min-h-0" : ""}`}>
      <div
        className={
          fitAspectRatio || surface === "hud"
            ? "w-full flex-1 min-h-0 flex items-center justify-center overflow-hidden"
            : "contents"
        }
      >
        <div
          ref={hostRef}
          data-native-preview-host
          className={
            surface === "hud"
              ? `pointer-events-none shrink-0 w-full h-full ${className}`
              : `pointer-events-none rounded-xl border border-studio-800 bg-black/50 ${fitAspectRatio ? "w-full h-full min-h-0" : `shrink-0 ${className}`}`
          }
          data-content-aspect-ratio={fitAspectRatio}
        />
      </div>
      {showStatus && <p className="text-xs text-studio-400 max-w-md text-center shrink-0">
        {error
          ? error
          : status?.attached
            ? live
              ? surface === "hud"
                ? "Live camera from the capture session"
                : "Live screen and selected webcam"
              : ""
            : "Native preview surface is not attached."}
      </p>}
    </div>
  );
}
