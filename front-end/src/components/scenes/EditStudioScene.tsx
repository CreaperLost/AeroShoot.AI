import { useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  Clapperboard,
  Clock,
  Folder,
  FolderOpen,
  MonitorPlay,
  Pencil,
  X,
} from "lucide-react";
import { TimelineStudio } from "../timeline/TimelineStudio";
import { NativePreviewHost } from "../canvas/NativePreviewHost";
import { InspectorPanel } from "../inspector/InspectorPanel";
import { useSettingsStore } from "../../stores/settingsStore";
import { useProjectStore } from "../../stores/projectStore";
import { api } from "../../lib/ipc";
import { ExportStatus, SegmentPage } from "../../lib/types";

function formatSeconds(us: number): string {
  return `${(us / 1_000_000).toFixed(2)}s`;
}

function getProjectFolderName(fullPath: string): string {
  const parts = fullPath.split(/[/\\]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : fullPath;
}

function shortenPath(fullPath: string): string {
  const parts = fullPath.split(/[/\\]/).filter(Boolean);
  if (parts.length <= 2) {
    return fullPath;
  }
  return `…/${parts.slice(-2).join("/")}`;
}

function getDefaultExportPath(bundlePath?: string | null, projectName?: string): string | undefined {
  if (!bundlePath) return undefined;
  const normalized = bundlePath.replace(/[/\\]+$/, "");
  const lastSlash = Math.max(normalized.lastIndexOf("/"), normalized.lastIndexOf("\\"));
  const parent = lastSlash >= 0 ? normalized.slice(0, lastSlash) : normalized;
  const name = (projectName || "Untitled").trim() || "Untitled";
  const safeName = name.replace(/[/\\:*?"<>|]/g, "").trim().replace(/^\.+/, "");
  const base = safeName.length > 0 ? safeName : "Untitled";
  const filename = base.toLowerCase().endsWith(".mp4") ? base : `${base}.mp4`;
  return `${parent}/${filename}`;
}

export function EditStudioScene() {
  const {
    openedProject: project,
    projectPath: path,
    recentProjects,
    loadOpenedProject,
    applyOpenedProject,
    clearProject,
    removeRecentProject,
    clearRecentProjects,
  } = useProjectStore();
  const { setActiveScene, canvas } = useSettingsStore();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [projectNameInput, setProjectNameInput] = useState("");
  const [isRenaming, setIsRenaming] = useState(false);
  const projectNameInputRef = useRef<HTMLInputElement>(null);
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
    setProjectNameInput(project?.manifest.projectName ?? "");
  }, [project?.projectHandle, project?.manifest.projectName]);

  const handleRename = async () => {
    if (!project || isRenaming) return;
    const trimmed = projectNameInput.trim();
    if (!trimmed) {
      setProjectNameInput(project.manifest.projectName);
      return;
    }
    if (trimmed === project.manifest.projectName) {
      return;
    }
    setIsRenaming(true);
    setError(undefined);
    try {
      const updated = await api.projectRename(project.projectHandle, trimmed);
      applyOpenedProject(updated);
    } catch (err) {
      setError(String(err));
      setProjectNameInput(project.manifest.projectName);
    } finally {
      setIsRenaming(false);
    }
  };

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

  const openPath = async (targetPath: string) => {
    setBusy(true);
    setError(undefined);
    try {
      // The backend replaces the current project only after the new one opens.
      // Keep both the editor and its live handle usable when validation fails.
      const opened = await api.openProject(targetPath);
      loadOpenedProject(opened, targetPath);
      setExportDestination("");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const defaultExportPath = getDefaultExportPath(path, project?.manifest.projectName);

  const chooseExportDestination = async () => {
    if (!project) return;
    setError(undefined);
    try {
      const picked = await api.pickExportDestination(project.projectHandle);
      if (!picked) return;
      setExportDestination(picked);
    } catch (err) {
      setError(String(err));
    }
  };

  const handleShowInFinder = async (targetPath?: string | null) => {
    const p = targetPath || path;
    if (!p) return;
    setError(undefined);
    try {
      await api.showInFinder(p);
    } catch (err) {
      setError(String(err));
    }
  };

  const open = async () => {
    setBusy(true);
    setError(undefined);
    try {
      const nextPath = await api.pickProjectFolder();
      if (!nextPath) {
        return;
      }
      await openPath(nextPath);
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
        {path && (
          <button
            type="button"
            disabled={busy}
            onClick={() => void handleShowInFinder(path)}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-studio-850/80 hover:bg-studio-800 border border-studio-750 text-studio-200 text-xs font-medium hover:text-white disabled:opacity-40 shrink-0 transition-colors"
            title={`Show ${path} in Finder`}
          >
            <Folder className="w-3.5 h-3.5 text-indigo-400 shrink-0" />
            <span>Show in Finder</span>
          </button>
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
        <button
          type="button"
          disabled={busy || !project || exporting}
          onClick={() => void chooseExportDestination()}
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-teal-600/20 border border-teal-500/40 text-teal-300 text-xs font-medium hover:bg-teal-600/30 disabled:opacity-40 shrink-0"
          title="Choose export save location"
        >
          <FolderOpen className="w-3.5 h-3.5" />
          Save as…
        </button>
        <span
          className="text-[11px] font-mono text-studio-400 truncate min-w-0 flex-1 self-center"
          title={exportDestination || defaultExportPath || "Choose export destination"}
        >
          {exportDestination || defaultExportPath || "Choose a project to export"}
        </span>
      </div>

      {error && (
        <p role="alert" className="px-4 py-2 text-sm text-rose-300 bg-rose-950/40 border-b border-rose-900/40">
          {error}
        </p>
      )}

      <div className="edit-studio-grid flex-1 min-h-0 overflow-hidden">
      <div className="min-h-0 overflow-hidden flex">
        <div className="flex-1 min-w-0 min-h-0 overflow-hidden p-5 flex flex-col gap-4">
        <div className="flex items-start justify-between gap-4 shrink-0">
          <div>
            {project ? (
              <div className="flex items-center gap-2 group">
                <input
                  ref={projectNameInputRef}
                  type="text"
                  aria-label="Project name"
                  value={projectNameInput}
                  disabled={isRenaming}
                  maxLength={80}
                  onChange={(e) => setProjectNameInput(e.target.value)}
                  onBlur={() => void handleRename()}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.currentTarget.blur();
                    } else if (e.key === "Escape") {
                      setProjectNameInput(project.manifest.projectName);
                      e.currentTarget.blur();
                    }
                  }}
                  className="text-lg text-white font-semibold bg-transparent hover:bg-studio-900/60 focus:bg-studio-900 border border-transparent hover:border-studio-700/60 focus:border-teal-500/80 rounded px-1.5 py-0.5 -ml-1.5 outline-none transition-colors max-w-md select-text"
                  title="Click to rename project"
                />
                <button
                  type="button"
                  aria-label="Rename project"
                  title="Rename project"
                  disabled={isRenaming}
                  onClick={() => {
                    projectNameInputRef.current?.focus();
                    projectNameInputRef.current?.select();
                  }}
                  className="p-1 rounded text-studio-500 hover:text-studio-300 hover:bg-studio-800/60 opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity"
                >
                  <Pencil className="w-3.5 h-3.5" />
                </button>
              </div>
            ) : (
              <h2 className="text-lg text-white font-semibold">
                No project open
              </h2>
            )}
            {project && (
              <p className="text-xs text-studio-400 mt-1">
                {project.tracks.length} tracks · source {formatSeconds(project.sourceDurationUs)} ·
                edited {formatSeconds(project.editedDurationUs)}
              </p>
            )}
          </div>
          {project && (
            <div className="flex items-center gap-2 shrink-0">
              {path && (
                <button
                  type="button"
                  onClick={() => void handleShowInFinder(path)}
                  className="flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-studio-850 hover:bg-studio-800 border border-studio-700 text-studio-300 hover:text-white text-xs font-medium transition-colors"
                  title={`Show ${path} in Finder`}
                >
                  <Folder className="w-3.5 h-3.5 text-indigo-400" />
                  <span>Show in Finder</span>
                </button>
              )}
              <span className="text-[10px] font-mono px-2 py-1 rounded bg-studio-800 text-studio-400">
                revision {project.revision}
              </span>
            </div>
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

        <div className="flex-1 min-h-0 overflow-hidden border border-studio-800 rounded-xl p-4 text-center bg-studio-900/40">
          {project ? (
            <div className="h-full min-h-0 flex flex-col items-center gap-3 text-studio-400">
              <NativePreviewHost
                key={project.projectHandle}
                fitAspectRatio={
                  canvas.aspectRatio === "9:16"
                    ? 9 / 16
                    : canvas.aspectRatio === "4:3"
                      ? 4 / 3
                      : canvas.aspectRatio === "1:1"
                        ? 1
                        : 16 / 9
                }
              />

            </div>
          ) : (
            <div className="h-full overflow-y-auto flex flex-col items-center gap-3 text-studio-400">
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

              {recentProjects.length > 0 && (
                <div className="w-full max-w-lg mt-6 pt-6 border-t border-studio-800 text-left">
                  <div className="flex items-center justify-between mb-3">
                    <span className="text-xs font-semibold uppercase tracking-wider text-studio-400 flex items-center gap-1.5">
                      <Clock className="w-3.5 h-3.5 text-studio-400" />
                      Recent projects
                    </span>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => clearRecentProjects()}
                      className="text-[11px] text-studio-500 hover:text-studio-300 transition-colors disabled:opacity-40"
                    >
                      Clear all
                    </button>
                  </div>
                  <ul className="space-y-2">
                    {recentProjects.map((recentPath) => {
                      const folderName = getProjectFolderName(recentPath);
                      const displayPath = shortenPath(recentPath);
                      return (
                        <li
                          key={recentPath}
                          className="flex items-center justify-between gap-3 px-3 py-2.5 rounded-lg bg-studio-900/90 hover:bg-studio-800/90 border border-studio-800 hover:border-studio-700 transition-colors group"
                        >
                          <div
                            className="flex items-center gap-2.5 min-w-0 flex-1 cursor-pointer"
                            onClick={() => void openPath(recentPath)}
                            title={recentPath}
                          >
                            <Folder className="w-4 h-4 text-teal-400 shrink-0" />
                            <div className="min-w-0 flex-1">
                              <p className="text-xs font-medium text-studio-200 group-hover:text-white truncate">
                                {folderName}
                              </p>
                              <p className="text-[11px] text-studio-500 font-mono truncate">
                                {displayPath}
                              </p>
                            </div>
                          </div>
                          <div className="flex items-center gap-1.5 shrink-0">
                            <button
                              type="button"
                              disabled={busy}
                              onClick={() => void openPath(recentPath)}
                              className="px-2.5 py-1 rounded bg-teal-600/20 hover:bg-teal-600/30 border border-teal-500/40 text-teal-300 text-xs font-medium hover:text-teal-200 disabled:opacity-40 transition-colors"
                            >
                              Open
                            </button>
                            <button
                              type="button"
                              disabled={busy}
                              onClick={(e) => {
                                e.stopPropagation();
                                void handleShowInFinder(recentPath);
                              }}
                              title={`Show ${recentPath} in Finder`}
                              className="px-2 py-1 rounded bg-studio-800 hover:bg-studio-700 border border-studio-700 text-studio-300 hover:text-white text-xs font-medium disabled:opacity-40 transition-colors"
                            >
                              Finder
                            </button>
                            <button
                              type="button"
                              disabled={busy}
                              onClick={(e) => {
                                e.stopPropagation();
                                removeRecentProject(recentPath);
                              }}
                              title="Remove from recent"
                              className="p-1 rounded text-studio-500 hover:text-studio-300 hover:bg-studio-800 opacity-60 group-hover:opacity-100 transition-opacity"
                            >
                              <X className="w-3.5 h-3.5" />
                            </button>
                          </div>
                        </li>
                      );
                    })}
                  </ul>
                </div>
              )}
            </div>
          )}
        </div>

        {project && (
        <details className="max-h-32 shrink-0 overflow-y-auto rounded-lg border border-studio-800 bg-studio-900/40 px-3 py-2 text-xs text-studio-300">
          <summary className="cursor-pointer font-medium text-studio-300">
            Recording diagnostics and segment details
          </summary>
          <div className="space-y-3 pt-3">
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
        </details>
        )}
      </div>
        {project && <InspectorPanel />}
      </div>

      <div className="min-h-0 border-t border-studio-800">
        <TimelineStudio />
      </div>
      </div>
    </div>
  );
}
