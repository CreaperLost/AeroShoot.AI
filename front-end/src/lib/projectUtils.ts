const MONTH_NAMES = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

/**
 * Returns the default dated project name for a blank/unnamed project,
 * e.g. "Untitled 9 Sep 2026". Matches the Rust backend default_project_name format.
 */
export function formatDatedUntitled(date: Date = new Date()): string {
  const day = date.getDate();
  const month = MONTH_NAMES[date.getMonth()];
  const year = date.getFullYear();
  return `Untitled ${day} ${month} ${year}`;
}

export function editedToSourceUs(
  retained: { startUs: number; endUs: number }[],
  editedUs: number,
): number | null {
  let accumulated = 0;
  for (const interval of retained) {
    const duration = interval.endUs - interval.startUs;
    if (editedUs < accumulated + duration) {
      return interval.startUs + (editedUs - accumulated);
    }
    accumulated += duration;
  }
  return null;
}
