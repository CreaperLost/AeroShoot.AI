import React, { useEffect, useState } from "react";
import { FolderOpen } from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { api } from "../../lib/ipc";
import { formatDatedUntitled } from "../../lib/projectUtils";
import { hostText } from "../../lib/platform";
import { SettingRow } from "../ui/controls";

const SAVE_DIR_KEY = "aeroshoot.projectSaveDir";

function shortenPath(path: string): string {
  const parts = path.split(/[/\\]/).filter(Boolean);
  if (parts.length <= 2) {
    return path;
  }
  return `…/${parts.slice(-2).join("/")}`;
}

/** Restore the saved recordings folder, or the platform default, once. */
export function useDefaultProjectDir() {
  const setProjectDir = useSettingsStore((state) => state.setProjectDir);
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
}

interface ProjectDestinationBarProps {
  disabled?: boolean;
}

/** Project name and recordings folder, as settings rows. */
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

  const locationLabel = projectDir ? shortenPath(projectDir) : "Documents/AeroShootRecordings";

  return (
    <div>
      <SettingRow title="Project name" description="Leave empty to name it by date.">
        <input
          type="text"
          aria-label="Project name"
          value={projectName}
          disabled={disabled}
          maxLength={80}
          placeholder={formatDatedUntitled()}
          onChange={(event) => setProjectName(event.target.value)}
          className="w-52 select-text rounded-lg border border-studio-700 bg-studio-950 px-2.5 py-1.5 text-[13px] text-white placeholder:text-studio-500 focus:border-indigo-500 focus:outline-none disabled:opacity-50"
        />
      </SettingRow>
      <SettingRow
        title="Folder"
        description={<span title={projectDir ?? undefined}>{locationLabel}</span>}
      >
        <button
          type="button"
          disabled={disabled}
          onClick={() => void chooseLocation()}
          title="Choose where new recordings are saved"
          className="flex items-center gap-1.5 rounded-lg border border-studio-700 bg-studio-850 px-2.5 py-1.5 text-[13px] text-studio-100 hover:bg-studio-800 disabled:opacity-50"
        >
          <FolderOpen aria-hidden="true" className="h-3.5 w-3.5 text-studio-400" />
          Change
        </button>
      </SettingRow>
      {createdProjectPath && (
        <button
          type="button"
          onClick={() => void handleShowInFinder()}
          title={`Show ${createdProjectPath} in ${hostText.fileManager}`}
          className="mt-1 text-xs text-indigo-300 hover:text-indigo-200"
        >
          Show last recording in {hostText.fileManager}
        </button>
      )}
      {pickerError && (
        <p role="alert" className="mt-1 text-xs text-rose-300">
          {pickerError}
        </p>
      )}
    </div>
  );
};
