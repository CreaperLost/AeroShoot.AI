import { useEffect, useState } from "react";
import { api } from "../../lib/ipc";
import type { MouseTelemetryPermission } from "../../lib/types";

export function MouseTelemetryControl({ disabled }: { disabled: boolean }) {
  const [status, setStatus] = useState<MouseTelemetryPermission>();
  const [busy, setBusy] = useState(false);
  const [requested, setRequested] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    let active = true;
    const refresh = () => {
      void api.mouseTelemetryPermission().then((next) => {
        if (active) { setStatus(next); setError(undefined); }
      }).catch(() => { if (active) setError("Mouse tracking permission could not be checked."); });
    };
    refresh();
    window.addEventListener("focus", refresh);
    return () => { active = false; window.removeEventListener("focus", refresh); };
  }, []);
  if (status && !status.supported) return null;
  const enable = async () => {
    setBusy(true); setRequested(true);
    try { setStatus(await api.mouseTelemetryPermission(true)); setError(undefined); }
    catch { setError("Open System Settings → Privacy & Security → Input Monitoring to enable AeroShoot."); }
    finally { setBusy(false); }
  };
  const message = error ?? (status?.authorized
    ? "Mouse tracking permitted · display and window recordings can log pointer motion for later editing."
    : requested
      ? "Recording still works. Enable AeroShoot in System Settings → Privacy & Security → Input Monitoring before your next recording to capture pointer motion."
      : "Recording still works without mouse tracking. Optional Input Monitoring records pointer movement and clicks for later editing.");
  return <div className="min-w-0 px-5 py-2 text-xs text-studio-300 bg-studio-950 border-b border-white/5 flex items-center gap-3 overflow-hidden z-20" aria-live="polite">
    <span className="min-w-0 truncate" title={message}>{message}</span>
    {!status?.authorized && !requested && <button type="button" disabled={disabled || busy || !status}
      onClick={() => void enable()} className="shrink-0 text-teal-400 disabled:opacity-40 hover:underline">
      {busy ? "Checking…" : "Enable mouse tracking"}
    </button>}
  </div>;
}
