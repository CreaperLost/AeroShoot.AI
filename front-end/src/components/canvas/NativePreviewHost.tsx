import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../../lib/ipc";

const WINDOW_LABEL = "main";

// The native surface is one per window. React StrictMode (development) mounts
// hosts twice; only the newest mount may detach it, or a stale mount's late
// attach reply would detach the surface its successor just attached.
let latestMount = 0;

interface NativePreviewHostProps {
  showStatus?: boolean;
  surfaceVisible?: boolean;
}

/**
 * Hosts the record-scene preview: an AppKit child view that mirrors this
 * element's rectangle above the WebView. Pixels never cross IPC here; this
 * component only reports geometry and visibility.
 */
export function NativePreviewHost({ showStatus = true, surfaceVisible = true }: NativePreviewHostProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const surfaceVisibleRef = useRef(surfaceVisible);
  const [error, setError] = useState<string>();

  // Visibility is layout state, not ownership state. Keep the native child
  // attached while it is hidden so asynchronous detach/attach calls cannot race
  // and resurrect an AppKit surface above web content.
  const scheduleLayoutRef = useRef<() => void>(() => undefined);
  useLayoutEffect(() => {
    surfaceVisibleRef.current = surfaceVisible;
    scheduleLayoutRef.current();
  }, [surfaceVisible]);

  useEffect(() => {
    const mount = ++latestMount;
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
      const viewport = { windowLabel: WINDOW_LABEL, x: rect.left, y: rect.top, width: rect.width, height: rect.height,
        backingScale: window.devicePixelRatio || 1, visible, occluded: !visible, generation, clip };
      const key = JSON.stringify(viewport);
      if (key === lastGeometry) return;
      sending = true;
      void api.studioPreviewLayout({ ...viewport, revision: ++revision })
        .then(() => { if (!cancelled) { lastGeometry = key; setError(undefined); } })
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
    void api.studioPreviewAttach(WINDOW_LABEL, "consume").then(() => api.studioPreviewStatus()).then(attached => {
      generation = attached.generation;
      if (cancelled) {
        if (mount === latestMount) void api.studioPreviewDetach().catch(() => undefined);
        return;
      }
      setError(undefined);
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
      if (generation !== undefined && mount === latestMount) {
        void api.studioPreviewDetach().catch(() => undefined);
      }
    };
  }, []);

  return (
    <div className="w-full flex-1 h-full min-h-0 flex flex-col items-center gap-2">
      <div className="preview-stage w-full flex-1 min-h-0 flex items-center justify-center overflow-hidden">
        {/* The AppKit view mirrors this box and draws above all web content,
            so the host must always fit inside its stage; a fixed-size box that
            overflows would paint over the rows above it. */}
        <div
          ref={hostRef}
          data-native-preview-host
          className="pointer-events-none preview-fit rounded-xl border border-studio-800 bg-black/50"
        />
      </div>
      {showStatus && error && (
        <p role="alert" className="text-xs text-rose-300 max-w-md text-center shrink-0">
          {error}
        </p>
      )}
    </div>
  );
}
