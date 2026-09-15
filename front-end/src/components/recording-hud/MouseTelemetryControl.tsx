import { useEffect, useState } from "react";
import { api } from "../../lib/ipc";
import type { MouseTelemetryPermission } from "../../lib/types";
import { useSettingsStore } from "../../stores/settingsStore";

export function MouseTelemetryControl({ disabled }: { disabled: boolean }) {
  const { captureMouse, setCaptureMouse } = useSettingsStore();
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
    document.addEventListener("visibilitychange", refresh);
    // Access is granted in System Settings, which may not refocus this window,
    // so keep re-checking until macOS reports it as granted.
    const timer = status?.authorized ? undefined : window.setInterval(refresh, 2000);
    return () => {
      active = false;
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [status?.authorized]);

  const allow = async () => {
    setBusy(true);
    setRequested(true);
    try {
      const next = await api.mouseTelemetryPermission(true);
      setStatus(next);
      setError(undefined);
      // macOS shows its prompt only once; after that, access is granted in System Settings.
      if (!next.authorized) await api.openSystemPrivacySettings("InputMonitoring");
    } catch {
      setError("Enable AeroShoot in System Settings → Privacy & Security → Input Monitoring.");
    } finally {
      setBusy(false);
    }
  };

  const supported = status?.supported ?? true;
  const needsPermission = captureMouse && supported && status !== undefined && !status.authorized;
  const message = !supported
    ? "Mouse tracking isn't available on this platform."
    : !captureMouse
      ? "Pointer moves and clicks won't be logged."
      : status?.authorized
        ? "Pointer moves and clicks are logged for editing."
        : !status
          ? "Checking Input Monitoring permission…"
          : requested
            ? "Enable AeroShoot under Input Monitoring in System Settings, then reopen AeroShoot. Recording still works meanwhile."
            : "Needs Input Monitoring permission. Recording still works without it.";

  return (
    <div className="flex min-w-0 flex-col gap-2 text-xs text-studio-300" aria-live="polite">
      <div
        role="radiogroup"
        aria-label="Mouse tracking"
        className="grid grid-cols-2 gap-0.5 rounded-lg border border-studio-800 bg-studio-950/60 p-0.5"
      >
        {[true, false].map((value) => (
          <button
            key={String(value)}
            type="button"
            role="radio"
            aria-checked={captureMouse === value}
            disabled={disabled || !supported}
            onClick={() => setCaptureMouse(value)}
            className={`rounded-md px-1 py-1 font-mono text-[11px] transition-colors disabled:opacity-50 ${
              captureMouse === value ? "bg-studio-800 text-white shadow-sm" : "text-studio-400 hover:text-studio-200"
            }`}
          >
            {value ? "On" : "Off"}
          </button>
        ))}
      </div>
      <p className={`leading-snug ${needsPermission || error ? "text-amber-300" : "text-studio-400"}`}>{error ?? message}</p>
      {needsPermission && (
        <button
          type="button"
          disabled={disabled || busy}
          onClick={() => void allow()}
          className="self-start rounded-lg border border-amber-700/50 bg-amber-900/30 px-2.5 py-1 font-medium text-amber-200 hover:bg-amber-900/50 disabled:opacity-50"
        >
          {busy ? "Requesting…" : "Allow Input Monitoring"}
        </button>
      )}
    </div>
  );
}
