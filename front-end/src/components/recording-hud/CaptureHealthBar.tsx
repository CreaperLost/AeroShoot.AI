import { AlertTriangle, Camera, Mic, Monitor, Volume2 } from "lucide-react";
import type { CaptureHealth } from "../../hooks/useRecording";

// The backend serializes a missing peak as `null`, so check with `== null`.
function Level({ db }: { db?: number | null }) {
  const known = db != null && Number.isFinite(db);
  const level = known ? Math.max(0, Math.min(100, ((db + 60) / 60) * 100)) : 0;
  return <span className="h-1.5 w-12 overflow-hidden rounded-full bg-studio-800" aria-label={known ? `${db.toFixed(1)} dBFS` : "No level"}>
    <span className={`block h-full rounded-full ${level > 92 ? "bg-rose-400" : level > 70 ? "bg-amber-400" : "bg-emerald-400"}`} style={{ width: `${level}%` }} />
  </span>;
}

function Source({ label, enabled, samples, segments, icon, db, ageMs, paused }: { label: string; enabled: boolean; samples: number; segments: number; icon: React.ReactNode; db?: number | null; ageMs?: number | null; paused?: boolean }) {
  const receiving = enabled && samples > 0 && (paused || (ageMs != null && ageMs < 2000));
  const status = !enabled ? "off" : paused && samples > 0 ? `paused · ${segments} saved` : receiving ? `${segments} saved` : samples > 0 ? "stalled" : "waiting";
  return <div className={`flex min-w-0 items-center gap-2 rounded-lg border px-2.5 py-1.5 ${enabled ? "border-studio-700 bg-studio-900/80" : "border-studio-850 bg-studio-950/50 opacity-55"}`}>
    <span className={receiving ? "text-emerald-400" : enabled ? "text-amber-400" : "text-studio-500"}>{icon}</span>
    <span className="text-[10px] font-semibold uppercase tracking-wide text-studio-300">{label}</span>
    {db !== undefined && <Level db={db} />}
    <span className={`ml-auto text-[10px] font-mono ${receiving ? "text-emerald-300" : "text-studio-500"}`}>
      {status}
    </span>
  </div>;
}

export function CaptureHealthBar({ health, screenEnabled, cameraEnabled, systemAudioEnabled, micEnabled, paused }: {
  health: CaptureHealth;
  screenEnabled: boolean;
  cameraEnabled: boolean;
  systemAudioEnabled: boolean;
  micEnabled: boolean;
  paused?: boolean;
}) {
  return <div className="w-full" aria-label="Live capture health">
    <div className="grid w-full grid-cols-1 gap-1.5">
      <Source label="Screen" enabled={screenEnabled} samples={health.screenSamples} segments={health.screenSegments} ageMs={health.screenLastSampleAgeMs} paused={paused} icon={<Monitor className="h-3.5 w-3.5" />} />
      <Source label="Camera" enabled={cameraEnabled} samples={health.cameraSamples} segments={health.cameraSegments} ageMs={health.cameraLastSampleAgeMs} paused={paused} icon={<Camera className="h-3.5 w-3.5" />} />
      <Source label="System" enabled={systemAudioEnabled} samples={health.systemAudioSamples} segments={health.systemAudioSegments} ageMs={health.systemAudioLastSampleAgeMs} paused={paused} db={health.systemAudioPeakDb} icon={<Volume2 className="h-3.5 w-3.5" />} />
      <Source label="Mic" enabled={micEnabled} samples={health.micSamples} segments={health.micSegments} ageMs={health.micLastSampleAgeMs} paused={paused} db={health.micPeakDb} icon={<Mic className="h-3.5 w-3.5" />} />
    </div>
    {health.firstTerminalError && (
      <div role="alert" className="mt-1.5 flex min-w-0 items-center gap-2 rounded-lg border border-rose-800/70 bg-rose-950/70 px-2.5 py-1.5 text-xs text-rose-200">
        <AlertTriangle aria-hidden="true" className="h-3.5 w-3.5 shrink-0 text-rose-400" />
        <span className="shrink-0 font-semibold">{health.firstTerminalError.trackId} failed</span>
        <span className="truncate" title={health.firstTerminalError.message}>{health.firstTerminalError.message}</span>
        <span className="ml-auto shrink-0 font-mono text-[10px] text-rose-400">{health.firstTerminalError.errorCode}</span>
      </div>
    )}
  </div>;
}
