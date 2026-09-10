import React, { useEffect, useState } from "react";
import { Folder, FolderOpen } from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { api } from "../../lib/ipc";
import { formatDatedUntitled } from "../../lib/projectUtils";

const SAVE_DIR_KEY = "aeroshoot.projectSaveDir";

function shortenPath(path: string): string {
  const parts = path.split(/[/\\]/).filter(Boolean);
  if (parts.length <= 2) {
    return path;
  }
  return `…/${parts.slice(-2).join("/")}`;
}

interface ProjectDestinationBarProps {
  disabled?: boolean;
}

export const ProjectDestinationBar: React.FC<ProjectDestinationBarProps> = ({ disabled = false }) => {
  const { projectName, projectDir, createdProjectPath, setProjectName, setProjectDir } = useSettingsStore();
  const [pickerError, setPickerError] = useState<string>();

  const handleShowInFinder = async () => {
    if (!createdProjectPath) return;
    setPickerError(undefined);
    try {
      await api.showInFinder(createdProjectPath);
    } catch (err) {
      setPickerError(String(err));
    }
  };

  useEffect(() => {
    let cancelled = false;
    const stored = typeof localStorage !== "undefined" ? localStorage.getItem(SAVE_DIR_KEY) : null;
    void api
      .getDefaultProjectsDir()
      .then((fallback) => {
        if (cancelled) return;
        const current = useSettingsStore.getState().projectDir;
        if (current) return;
        const next = stored && stored.length > 0 ? stored : fallback;
        if (next) setProjectDir(next);
      })
      .catch(() => {
        if (!cancelled && stored) setProjectDir(stored);
      });
    return () => {
      cancelled = true;
    };
  }, [setProjectDir]);

  const chooseLocation = async () => {
    setPickerError(undefined);
    try {
      const picked = await api.pickSaveDirectory();
      if (!picked) return;
      setProjectDir(picked);
      localStorage.setItem(SAVE_DIR_KEY, picked);
    } catch (err) {
      setPickerError(String(err));
    }
  };

  const locationLabel = projectDir ? shortenPath(projectDir) : "Documents/AeroShootRec";

  return (
    <div className="project-destination-grid w-full bg-studio-900/70 border-b border-studio-800/80 px-5 py-2 text-xs z-20">
      <label className="flex items-center gap-2 min-w-0 flex-1">
        <span className="uppercase font-semibold tracking-wider text-studio-400 shrink-0">Project</span>
        <input
          type="text"
          value={projectName}
          disabled={disabled}
          maxLength={80}
          placeholder={formatDatedUntitled()}
          onChange={(event) => setProjectName(event.target.value)}
          className="select-text min-w-0 flex-1 px-3 py-1.5 rounded-lg bg-studio-950 border border-studio-750 text-white placeholder:text-studio-500 focus:outline-none focus:border-indigo-500/70 disabled:opacity-50"
        />
      </label>
      <button
        type="button"
        disabled={disabled}
        onClick={() => void chooseLocation()}
        title={projectDir ?? "Choose where new .aero folders are created"}
        className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-studio-850/80 hover:bg-studio-800 border border-studio-750 text-studio-200 disabled:opacity-50 max-w-full"
      >
        <FolderOpen className="w-3.5 h-3.5 text-indigo-400 shrink-0" />
        <span className="truncate">{locationLabel}</span>
        <Folder className="w-3.5 h-3.5 text-studio-500 shrink-0" />
      </button>
      {createdProjectPath && (
        <button
          type="button"
          onClick={() => void handleShowInFinder()}
          title={`Show ${createdProjectPath} in Finder`}
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-indigo-600/20 hover:bg-indigo-600/30 border border-indigo-500/40 text-indigo-300 text-xs font-medium shrink-0 transition-colors shadow-sm"
        >
          <Folder className="w-3.5 h-3.5 text-indigo-400 shrink-0" />
          <span>Show in Finder</span>
        </button>
      )}
      {pickerError && (
        <span role="alert" className="text-rose-300 truncate max-w-full">
          {pickerError}
        </span>
      )}
    </div>
  );
};
