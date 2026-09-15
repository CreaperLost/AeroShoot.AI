import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../../lib/ipc";
import { PreviewHitMode, PreviewStatus } from "../../lib/types";

interface NativePreviewHostProps {
  live?: boolean;
  windowLabel?: string;
  hitMode?: PreviewHitMode;
  className?: string;
  surface?: "studio" | "hud";
  showStatus?: boolean;
  surfaceVisible?: boolean;
}

export function NativePreviewHost({
  live = true,
  windowLabel = "main",
  hitMode = "consume",
  className = "w-full max-w-4xl aspect-video",
  surface = "studio",
  showStatus = true,
  surfaceVisible = true,
}: NativePreviewHostProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const surfaceVisibleRef = useRef(surfaceVisible);
  const [status, setStatus] = useState<PreviewStatus | null>(null);
  const [error, setError] = useState<string>();

  // The studio (`main` window) and HUD (`camera_overlay` window) surfaces are
  // owned by independent `PreviewOwner` instances on the Rust side and exposed
  // through separate Tauri command sets. Route attach / layout / status /
  // detach calls accordingly so the host works in both windows without
  // window-label-rejection errors.
  const isHud = windowLabel === "camera_overlay";

  // Visibility is layout state, not ownership state. Keep the native child
  // attached while menus open so asynchronous detach/attach calls cannot race
  // and resurrect an AppKit surface above the WebView dropdown.
  const scheduleLayoutRef = useRef<() => void>(() => undefined);
  useLayoutEffect(() => {
    surfaceVisibleRef.current = surfaceVisible;
    scheduleLayoutRef.current();
  }, [surfaceVisible]);

  useEffect(() => {
    let cancelled = false;
    let revision = 0;
    let generation: number | undefined;
    let frame = 0;
    let lastGeometry = "";
    let sending = false;
    let dirty = false;
    // Measure on demand (resize, scroll, visibility, a slow fallback) instead of
    // walking computed styles on every animation frame, which kept the WebView busy.
    const schedule = () => {
      if (cancelled || frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        update();
      });
    };
    const update = () => {
      const el = hostRef.current;
      if (cancelled || !el || generation === undefined) return;
      if (sending) {
        dirty = true;
        return;
      }
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
      const visible = surfaceVisibleRef.current && !document.hidden && clip[2] > 0 && clip[3] > 0;
      const viewport = { windowLabel, x: rect.left, y: rect.top, width: rect.width, height: rect.height,
        backingScale: window.devicePixelRatio || 1, visible, occluded: !visible, generation, clip };
      const key = JSON.stringify(viewport);
      if (key === lastGeometry) return;
      sending = true;
      const layout = isHud ? api.hudPreviewLayout : api.studioPreviewLayout;
      void layout({ ...viewport, revision: ++revision })
        .then(next => { if (!cancelled) { lastGeometry = key; setStatus(next); setError(undefined); } })
        .catch(err => { if (!cancelled) setError(String(err)); })
        .finally(() => {
          sending = false;
          if (dirty) {
            dirty = false;
            schedule();
          }
        });
    };
    scheduleLayoutRef.current = schedule;
    const resizeObserver = new ResizeObserver(schedule);
    resizeObserver.observe(document.documentElement);
    if (hostRef.current) resizeObserver.observe(hostRef.current);
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    document.addEventListener("visibilitychange", schedule);
    // Catches moves that change neither size nor scroll, e.g. a banner above the host.
    const fallback = window.setInterval(schedule, 500);
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
      schedule();
    }).catch(err => { if (!cancelled) setError(String(err)); });
    return () => {
      cancelled = true;
      cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      document.removeEventListener("visibilitychange", schedule);
      window.clearInterval(fallback);
      scheduleLayoutRef.current = () => undefined;
      if (generation !== undefined) {
        void (isHud ? api.hudClose() : api.studioPreviewDetach()).catch(() => undefined);
      }
    };
  }, [windowLabel, hitMode, surface, isHud]);

  return (
    <div className="w-full flex-1 h-full min-h-0 flex flex-col items-center gap-2">
      <div
        className={
          surface === "hud"
            ? "w-full flex-1 min-h-0 flex items-center justify-center overflow-hidden"
            : "preview-stage w-full flex-1 min-h-0 flex items-center justify-center overflow-hidden"
        }
      >
        {/* The AppKit view mirrors this box and draws above all web content,
            so the studio host must always fit inside its stage; a fixed-size
            box that overflows would paint over the rows above it. */}
        <div
          ref={hostRef}
          data-native-preview-host
          className={
            surface === "hud"
              ? `pointer-events-none shrink-0 w-full h-full ${className}`
              : "pointer-events-none preview-fit rounded-xl border border-studio-800 bg-black/50"
          }
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
