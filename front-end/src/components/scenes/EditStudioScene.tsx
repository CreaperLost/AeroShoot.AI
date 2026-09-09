import { useEffect, useState } from "react";
import {
  AlertTriangle,
  Clapperboard,
  FolderOpen,
  MonitorPlay,
} from "lucide-react";
import { TimelineStudio } from "../timeline/TimelineStudio";
import { NativePreviewHost } from "../canvas/NativePreviewHost";
import { useSettingsStore } from "../../stores/settingsStore";
import { useProjectStore } from "../../stores/projectStore";
import { api } from "../../lib/ipc";
import { ExportStatus, SegmentPage } from "../../lib/types";

function formatSeconds(us: number): string {
  return `${(us / 1_000_000).toFixed(2)}s`;
}

export function EditStudioScene() {
  const { openedProject: project, loadOpenedProject, clearProject } = useProjectStore();
  const { setActiveScene } = useSettingsStore();
  const [path, setPath] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [trackId, setTrackId] = useState("");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<SegmentPage>();
  const [resolution, setResolution] = useState("1920x1080");
  const [exportFps, setExportFps] = useState(30);
  const [exportDestination, setExportDestination] = useState("");
  const [exportJob, setExportJob] = useState<ExportStatus>();
  const [defaultProjectsDir, setDefaultProjectsDir] = useState<string>();

  useEffect(() => {
    void api.getDefaultProjectsDir().then(setDefaultProjectsDir).catch(() => {});
  }, []);

  useEffect(() => {
    setTrackId(project?.tracks[0]?.descriptor.id ?? "");
    setOffset(0);
  }, [project?.projectHandle]);

  useEffect(() => {
    let active = true;
    setPage(undefined);
    if (project && trackId) {
      void api
        .projectSegments(project.projectHandle, trackId, offset)
        .then((next) => {
          if (active) setPage(next);
        })
        .catch((err) => {
          if (active) setError(String(err));
        });
    }
    return () => {
      active = false;
    };
  }, [project?.projectHandle, trackId, offset]);

  useEffect(() => {
    if (!exportJob || (exportJob.state !== "queued" && exportJob.state !== "running")) {
      return;
    }
    let active = true;
    const timer = window.setInterval(() => {
      void api
        .exportStatus(exportJob.jobId)
        .then((next) => {
          if (active) setExportJob(next);
        })
        .catch((err) => {
          if (active) setError(String(err));
        });
    }, 250);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [exportJob?.jobId, exportJob?.state]);

  const exporting = exportJob?.state === "queued" || exportJob?.state === "running";

  const open = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const nextPath = await api.pickProjectFolder();
      if (!nextPath) {
        return;
      }
      setPath(nextPath);
      // The backend replaces the current project only after the new one opens.
      // Keep both the editor and its live handle usable when validation fails.
      loadOpenedProject(await api.openProject(nextPath));
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const close = async () => {
    if (!project) return;
    setBusy(true);
    setError(undefined);
    try {
      await api.closeProject(project.projectHandle);
      clearProject();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const startExport = async () => {
    if (!project) return;
    setError(undefined);
    try {
      const next = await api.exportStart(project.projectHandle, {
        videoCodec: "h264",
        audioCodec: "aac",
        width: Number(resolution.split("x")[0]),
        height: Number(resolution.split("x")[1]),
        fps: exportFps,
        destination: exportDestination.trim() || undefined,
      });
      setExportJob(next);
      if (next.failure) {
        setError(`${next.failure.kind}: ${next.failure.message}`);
      }
    } catch (err) {
      setError(String(err));
    }
  };

  const cancelExport = async () => {
    if (!exportJob?.jobId) return;
    try {
      setExportJob(await api.exportCancel(exportJob.jobId));
    } catch (err) {
      setError(String(err));
    }
  };

  return (
    <div className="flex-1 flex flex-col min-w-0 overflow-hidden bg-studio-950">
      <div className="px-4 py-3 border-b border-studio-800 flex gap-3 items-center bg-studio-900/80">
        <button
          onClick={() => setActiveScene("record")}
          className="text-xs text-studio-300 hover:text-white shrink-0"
        >
          New recording
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void open()}
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-teal-600/20 border border-teal-500/40 text-teal-300 text-xs font-medium hover:bg-teal-600/30 disabled:opacity-40 shrink-0"
        >
          <FolderOpen className="w-3.5 h-3.5" />
          Open folder
        </button>
        {path ? (
          <span className="text-[11px] font-mono text-studio-400 truncate min-w-0 flex-1" title={path}>
            {path}
          </span>
        ) : (
          <span className="text-[11px] text-studio-500 truncate min-w-0 flex-1">
            {defaultProjectsDir
              ? `Choose a project folder (default: ${defaultProjectsDir})`
              : "Choose a .aero project folder"}
          </span>
        )}
        <button
          disabled={busy || !project}
          onClick={() => void close()}
          className="text-xs text-studio-300 disabled:opacity-40 shrink-0"
        >
          Close project
        </button>
        <button
          disabled={busy || !project || exporting}
          onClick={() => void startExport()}
          title="Export MP4 with H.264 video and stereo AAC audio"
          className="text-xs text-teal-400 disabled:opacity-40 shrink-0"
        >
          {exporting ? "Exporting…" : "Export"}
        </button>
        {exporting && (
          <button
            onClick={() => void cancelExport()}
            className="text-xs text-studio-300 shrink-0"
          >
            Cancel export
          </button>
        )}
        <button
          disabled
          title="Real audio analysis is not implemented"
          className="text-xs opacity-40 shrink-0"
        >
          AI silence cuts
        </button>
      </div>

      <div className="flex flex-wrap gap-3 px-4 py-2 border-b border-studio-800 text-xs">
        <label>Export size <select aria-label="Export size" disabled={exporting} value={resolution} onChange={e=>setResolution(e.target.value)} className="bg-studio-800 p-1 rounded">
          <option value="1280x720">720p</option><option value="1920x1080">1080p</option><option value="3840x2160">4K</option>
        </select></label>
        <label>Frame rate <select aria-label="Export frame rate" disabled={exporting} value={exportFps} onChange={e=>setExportFps(Number(e.target.value))} className="bg-studio-800 p-1 rounded">
          <option value={24}>24 fps</option><option value={30}>30 fps</option><option value={60}>60 fps</option>
        </select></label>
        <input
          aria-label="Export destination"
          disabled={exporting}
          placeholder={defaultProjectsDir ? `Output path (default: ${defaultProjectsDir}/...)` : "Output path (optional)"}
          value={exportDestination}
          onChange={e=>setExportDestination(e.target.value)}
          className="bg-studio-800 p-1 rounded flex-1 min-w-48"
        />
      </div>

      {error && (
        <p role="alert" className="px-4 py-2 text-sm text-rose-300 bg-rose-950/40 border-b border-rose-900/40">
          {error}
        </p>
      )}

      <div className="flex-1 min-h-0 overflow-auto p-5 space-y-4">
        <div className="flex items-start justify-between gap-4">
          <div>
            <h2 className="text-lg text-white font-semibold">
              {project?.manifest.projectName ?? "No project open"}
            </h2>
            {project && (
              <p className="text-xs text-studio-400 mt-1">
                {project.tracks.length} tracks · source {formatSeconds(project.sourceDurationUs)} ·
                edited {formatSeconds(project.editedDurationUs)}
              </p>
            )}
          </div>
          {project && (
            <span className="text-[10px] font-mono px-2 py-1 rounded bg-studio-800 text-studio-400">
              revision {project.revision}
            </span>
          )}
        </div>

        {exportJob && exportJob.state !== "idle" && (
          <p className="text-xs text-studio-300">
            Export {exportJob.state}
            {exportJob.progressDenominator > 0
              ? ` · ${exportJob.progressNumerator}/${exportJob.progressDenominator}`
              : ""}
            {exportJob.outputPath ? ` · ${exportJob.outputPath}` : ""}
            {exportJob.failure ? ` · ${exportJob.failure.kind}: ${exportJob.failure.message}` : ""}
            {exportJob.state === "completed"
              ? " · verified and saved"
              : ""}
          </p>
        )}

        <div className="border border-studio-800 rounded-xl p-8 text-center bg-studio-900/40">
          {project ? (
            <div className="flex flex-col items-center gap-3 text-studio-400">
              <NativePreviewHost key={project.projectHandle} />

            </div>
          ) : (
            <div className="flex flex-col items-center gap-3 text-studio-400">
              <MonitorPlay className="w-8 h-8 text-studio-500" />
              <p className="text-sm text-studio-200">Empty editor</p>
              <p className="text-xs max-w-md">
                Record a session or open an existing project folder to inspect its tracks. No sample
                timeline is loaded.
              </p>
              <button
                type="button"
                disabled={busy}
                onClick={() => void open()}
                className="mt-1 flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-teal-600/20 border border-teal-500/40 text-teal-300 text-xs font-medium hover:bg-teal-600/30 disabled:opacity-40"
              >
                <FolderOpen className="w-3.5 h-3.5" />
                Open folder
              </button>
            </div>
          )}
        </div>

        {project && project.diagnostics.length > 0 && (
          <ul className="text-xs text-amber-300 space-y-1 bg-amber-950/20 border border-amber-900/40 rounded-lg p-3">
            {project.diagnostics.map((message, i) => (
              <li key={i} className="flex gap-2">
                <AlertTriangle className="w-3.5 h-3.5 shrink-0 mt-0.5" />
                <span>{message}</span>
              </li>
            ))}
          </ul>
        )}

        {project && project.tracks.length === 0 && (
          <p className="text-xs text-studio-400">This project has no tracks.</p>
        )}

        {project && project.tracks.length > 0 && (
          <div className="space-y-3">
            <label className="text-xs text-studio-300 flex items-center gap-3">
              <span className="flex items-center gap-1.5">
                <Clapperboard className="w-3.5 h-3.5" />
                Inspect segments
              </span>
              <select
                aria-label="Track segments"
                value={trackId}
                onChange={(e) => {
                  setTrackId(e.target.value);
                  setOffset(0);
                }}
                className="bg-studio-800 p-1.5 rounded-md text-studio-100"
              >
                {project.tracks.map((t) => (
                  <option key={t.descriptor.id} value={t.descriptor.id}>
                    {t.descriptor.id} — {t.availableSegmentCount}/{t.segmentCount} available
                  </option>
                ))}
              </select>
            </label>

            <table className="w-full text-xs text-left">
              <thead>
                <tr className="text-studio-400 border-b border-studio-800">
                  <th className="py-2 font-medium">Segment</th>
                  <th className="py-2 font-medium">Source interval</th>
                  <th className="py-2 font-medium">Status</th>
                </tr>
              </thead>
              <tbody>
                {page?.segments.map((s) => (
                  <tr key={s.relativePath} className="border-b border-studio-800/50">
                    <td className="py-2 font-mono text-studio-200">{s.relativePath}</td>
                    <td className="py-2 text-studio-300">
                      {(s.startUs / 1e6).toFixed(3)}–{(s.endUs / 1e6).toFixed(3)}s
                    </td>
                    <td className={s.available ? "text-teal-400" : "text-amber-300"}>
                      {s.available ? "Present (decode unverified)" : "Unavailable / conflicting"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            {page?.segments.length === 0 && (
              <p className="text-xs text-studio-400">No committed segments.</p>
            )}
            <div className="flex gap-4 text-xs">
              <button
                disabled={offset === 0}
                onClick={() => setOffset(Math.max(0, offset - 100))}
                className="text-studio-300 disabled:opacity-40"
              >
                Previous
              </button>
              <button
                disabled={page?.nextOffset == null}
                onClick={() => {
                  if (page?.nextOffset != null) setOffset(page.nextOffset);
                }}
                className="text-studio-300 disabled:opacity-40"
              >
                Next
              </button>
            </div>
          </div>
        )}
      </div>

      <div className="h-72 shrink-0 border-t border-studio-800">
        <TimelineStudio />
      </div>
    </div>
  );
}
