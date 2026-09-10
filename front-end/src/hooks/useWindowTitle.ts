import { useEffect } from "react";
import { useSettingsStore } from "../stores/settingsStore";
import { useProjectStore } from "../stores/projectStore";
import { api } from "../lib/ipc";
import { formatDatedUntitled } from "../lib/projectUtils";

export function formatWindowTitle(
  activeScene: "record" | "edit",
  projectName: string,
  openedProjectName?: string | null,
  now: Date = new Date(),
): string {
  if (activeScene === "record") {
    const trimmed = projectName.trim();
    const name = trimmed || formatDatedUntitled(now);
    return `AeroShoot \u2014 ${name}`;
  }
  const trimmed = openedProjectName?.trim();
  return trimmed ? `AeroShoot \u2014 ${trimmed}` : "AeroShoot";
}

export function useWindowTitle(): void {
  const activeScene = useSettingsStore((s) => s.activeScene);
  const projectName = useSettingsStore((s) => s.projectName);
  const openedProjectName = useProjectStore((s) => s.openedProject?.manifest.projectName);

  useEffect(() => {
    const title = formatWindowTitle(activeScene, projectName, openedProjectName);
    void api.setWindowTitle(title);
  }, [activeScene, projectName, openedProjectName]);
}
