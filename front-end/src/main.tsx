import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { HudOnlyRoot, RejectedWindowRoot } from "./components/camera-overlay/HudOnlyRoot";
import { api, isTauriEnvironment } from "./lib/ipc";
import { selectUiRoot, UiMount } from "./lib/windowIdentity";
import "./index.css";

async function resolveMount(): Promise<UiMount> {
  const hash = window.location.hash;
  if (!isTauriEnvironment()) {
    return selectUiRoot({ identity: null, hash, tauri: false });
  }
  try {
    const identity = await api.windowIdentity();
    const mount = selectUiRoot({ identity, hash, tauri: true });
    if (mount === "hud" && hash !== "#overlay") {
      window.history.replaceState(null, "", "#overlay");
    }
    return mount;
  } catch {
    return "rejected";
  }
}

void resolveMount().then((mount) => {
  const root = document.getElementById("root");
  if (!root) return;
  const tree =
    mount === "hud" ? (
      <HudOnlyRoot />
    ) : mount === "rejected" ? (
      <RejectedWindowRoot />
    ) : (
      <App />
    );
  ReactDOM.createRoot(root).render(<React.StrictMode>{tree}</React.StrictMode>);
});
