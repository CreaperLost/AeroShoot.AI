import { WindowIdentity, UiRootKind } from "./types";

export type UiMount = "studio" | "hud" | "rejected";

/**
 * Select the React root from a verified window identity.
 * Hash `#overlay` is reinforcement only; a Tauri label always wins.
 * Unknown privileged identities must not receive the studio.
 */
export function selectUiRoot(args: {
  identity: WindowIdentity | null;
  hash: string;
  tauri: boolean;
}): UiMount {
  if (args.tauri) {
    if (!args.identity || args.identity.rejected || !args.identity.uiRoot) {
      return "rejected";
    }
    return args.identity.uiRoot === "hud" ? "hud" : "studio";
  }
  const hash = args.hash.replace(/\/$/, "");
  if (hash === "#overlay") {
    return "hud";
  }
  return "studio";
}

export function identityFromLabel(label: string): WindowIdentity {
  if (label === "main") {
    return { label, uiRoot: "studio" satisfies UiRootKind, rejected: false };
  }
  if (label === "camera_overlay") {
    return { label, uiRoot: "hud", rejected: false };
  }
  return { label, uiRoot: null, rejected: true };
}
