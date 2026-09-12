import { useEffect } from "react";
import { useSettingsStore } from "../stores/settingsStore";
import { api } from "../lib/ipc";
import { formatDatedUntitled } from "../lib/projectUtils";

export function formatWindowTitle(
  projectName: string,
  now: Date = new Date(),
): string {
  const trimmed = projectName.trim();
  const name = trimmed || formatDatedUntitled(now);
  return `AeroShoot \u2014 ${name}`;
}

export function useWindowTitle(): void {
  const projectName = useSettingsStore((s) => s.projectName);

  useEffect(() => {
    const title = formatWindowTitle(projectName);
    void api.setWindowTitle(title);
  }, [projectName]);
}
