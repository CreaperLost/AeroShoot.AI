import { useEffect } from "react";
import { api, isTauriEnvironment } from "../lib/ipc";
import { HudSnapshot } from "../lib/types";
import { useSettingsStore } from "../stores/settingsStore";

const HUD_EVENT = "hud-settings";

export function applyHudEvent(lastSeen: number, incoming: HudSnapshot): "apply" | "snapshot" {
  if (incoming.revision !== lastSeen + 1) {
    return "snapshot";
  }
  return "apply";
}

export function useHudSettings(): void {
  const applyHudSnapshot = useSettingsStore((s) => s.applyHudSnapshot);

  useEffect(() => {
    let cancelled = false;
    let lastSeen = useSettingsStore.getState().hudRevision;

    const hydrate = (snapshot: HudSnapshot) => {
      lastSeen = snapshot.revision;
      applyHudSnapshot(snapshot);
    };

    void api.hudSnapshot().then((snapshot) => {
      if (!cancelled) hydrate(snapshot);
    });

    let unlisten: (() => void) | undefined;
    if (isTauriEnvironment()) {
      void import("@tauri-apps/api/event")
        .then(({ listen }) =>
          listen<HudSnapshot>(HUD_EVENT, (event) => {
            if (cancelled) return;
            if (applyHudEvent(lastSeen, event.payload) === "snapshot") {
              void api.hudSnapshot().then((snapshot) => {
                if (!cancelled) hydrate(snapshot);
              });
              return;
            }
            hydrate(event.payload);
          }),
        )
        .then((fn) => {
          unlisten = fn;
        })
        .catch(() => undefined);
    }

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [applyHudSnapshot]);
}
