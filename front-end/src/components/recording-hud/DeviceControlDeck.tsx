import React, { useState, useRef, useEffect, useLayoutEffect } from "react";
import { Monitor, Camera, Mic, Volume2, ChevronDown, Check } from "lucide-react";
import { useSettingsStore } from "../../stores/settingsStore";
import { CaptureSource, CameraDevice, AudioDevice, PermissionBundle } from "../../lib/types";
import { MicGainSlider } from "./MicGainSlider";
import { api } from "../../lib/ipc";
import { hostText } from "../../lib/platform";
import { Switch } from "../ui/controls";

interface DeviceControlDeckProps {
  sources: CaptureSource[];
  cameras: CameraDevice[];
  mics: AudioDevice[];
  disabled?: boolean;
  onDropdownOpenChange?: (open: boolean) => void;
  onRequestCameraPermission?: () => Promise<void>;
  onRequestMicrophonePermission?: () => Promise<void>;
  permissions: PermissionBundle;
  needsScreenPermission: boolean;
  onRequestScreenPermission?: () => Promise<void>;
}

type Menu = "source" | "mic" | "camera";

const levelPercent = (peakDb: number | null) =>
  peakDb === null ? 0 : Math.max(0, Math.min(100, ((peakDb + 60) / 60) * 100));

function LevelMeter({ peakDb, label }: { peakDb: number | null; label: string }) {
  return (
    <span
      className="mt-1.5 block h-1 w-full overflow-hidden rounded-full bg-studio-800"
      title={peakDb === null ? `Waiting for ${label}` : `${peakDb.toFixed(1)} dBFS`}
    >
      <span
        className={`block h-full rounded-full ${peakDb !== null && peakDb > -3 ? "bg-rose-400" : "bg-emerald-400"}`}
        style={{ width: `${levelPercent(peakDb)}%` }}
      />
    </span>
  );
}

/** One source: name and device on the left (opens its menu), on/off switch on the right. */
function SourceRow({
  icon,
  title,
  detail,
  on,
  onToggle,
  switchLabel,
  disabled,
  switchDisabled,
  menu,
  open,
  onOpen,
  menuId,
  children,
  issue,
}: {
  icon: React.ReactNode;
  title: string;
  detail: React.ReactNode;
  on: boolean;
  onToggle: (on: boolean) => void;
  switchLabel: string;
  disabled: boolean;
  switchDisabled?: boolean;
  menu?: Menu;
  open?: boolean;
  onOpen?: () => void;
  menuId?: string;
  children?: React.ReactNode;
  issue?: React.ReactNode;
}) {
  const body = (
    <>
      <span aria-hidden="true" className={on ? "text-studio-100" : "text-studio-500"}>
        {icon}
      </span>
      <span className="min-w-0 flex-1 text-left">
        <span className={`block text-[13px] font-medium ${on ? "text-white" : "text-studio-400"}`}>{title}</span>
        <span className={`block truncate text-xs ${on ? "text-studio-400" : "text-studio-500"}`}>{detail}</span>
      </span>
    </>
  );
  return (
    <div className="relative min-w-0">
      <div
        className={`flex items-center gap-3 rounded-lg border px-3 py-2.5 transition-colors ${
          open ? "border-indigo-500/70 bg-studio-800" : on ? "border-studio-700 bg-studio-850" : "border-studio-800 bg-studio-900"
        }`}
      >
        {menu ? (
          <button
            type="button"
            disabled={disabled}
            aria-haspopup="menu"
            aria-expanded={open}
            aria-controls={menuId}
            aria-label={`${title}: choose device`}
            onClick={onOpen}
            className="flex min-w-0 flex-1 items-center gap-3 rounded-md disabled:cursor-not-allowed"
          >
            {body}
            <ChevronDown
              aria-hidden="true"
              className={`h-3.5 w-3.5 shrink-0 text-studio-500 transition-transform ${open ? "rotate-180" : ""}`}
            />
          </button>
        ) : (
          <div className="flex min-w-0 flex-1 items-center gap-3">{body}</div>
        )}
        <Switch checked={on} onChange={onToggle} label={switchLabel} disabled={disabled || switchDisabled} />
      </div>
      {children}
      {issue}
    </div>
  );
}

function MenuPanel({ id, label, children }: { id: string; label: string; children: React.ReactNode }) {
  return (
    <div
      id={id}
      role="menu"
      aria-label={label}
      className="device-dropdown-panel absolute inset-x-0 top-full z-50 mt-1.5 rounded-xl border border-studio-700 bg-studio-900 p-1.5 shadow-2xl"
    >
      {children}
    </div>
  );
}

function MenuItem({
  selected,
  onSelect,
  children,
}: {
  selected: boolean;
  onSelect: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      role="menuitemradio"
      aria-checked={selected}
      onClick={onSelect}
      className={`flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-2 text-left text-[13px] transition-colors ${
        selected ? "bg-indigo-500/15 text-white" : "text-studio-200 hover:bg-studio-800"
      }`}
    >
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {selected && <Check aria-hidden="true" className="h-3.5 w-3.5 shrink-0 text-indigo-300" />}
    </button>
  );
}

export const DeviceControlDeck: React.FC<DeviceControlDeckProps> = ({
  sources,
  cameras,
  mics,
  disabled = false,
  onDropdownOpenChange,
  onRequestCameraPermission,
  onRequestMicrophonePermission,
  permissions,
  needsScreenPermission,
  onRequestScreenPermission,
}) => {
  const settings = useSettingsStore();

  const [openDropdown, setOpenDropdown] = useState<Menu | null>(null);
  const [micPeakDb, setMicPeakDb] = useState<number | null>(null);
  const [systemPeakDb, setSystemPeakDb] = useState<number | null>(null);

  const containerRef = useRef<HTMLDivElement | null>(null);

  // Close the open menu on an outside click.
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(event.target as Node)) {
        setOpenDropdown(null);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  useEffect(() => {
    if (!openDropdown) return;
    window.requestAnimationFrame(() => {
      containerRef.current
        ?.querySelector<HTMLElement>("[role='menu'] [role^='menuitem']")
        ?.focus();
    });
  }, [openDropdown]);

  const handleMenuKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (!openDropdown || !["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const items = Array.from(
      containerRef.current?.querySelectorAll<HTMLElement>("[role='menu'] [role^='menuitem']:not([disabled])") ?? [],
    );
    if (items.length === 0) return;
    event.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLElement);
    const next = event.key === "Home" ? 0
      : event.key === "End" ? items.length - 1
      : event.key === "ArrowUp" ? (current <= 0 ? items.length - 1 : current - 1)
      : (current + 1) % items.length;
    items[next]?.focus();
  };

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpenDropdown(null);
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, []);

  useLayoutEffect(() => {
    onDropdownOpenChange?.(openDropdown !== null);
    return () => onDropdownOpenChange?.(false);
  }, [openDropdown, onDropdownOpenChange]);

  useEffect(() => {
    if ((!settings.captureSystemAudio && !settings.selectedMicId) || disabled) {
      setSystemPeakDb(null);
      setMicPeakDb(null);
      return;
    }
    let active = true;
    const poll = () => void api.capturePreviewAudioLevels().then((levels) => {
      if (active) {
        setSystemPeakDb(levels.systemAudioPeakDb ?? null);
        setMicPeakDb(levels.micPeakDb ?? null);
      }
    }).catch(() => {
      if (active) { setSystemPeakDb(null); setMicPeakDb(null); }
    });
    poll();
    const timer = window.setInterval(poll, 150);
    return () => { active = false; window.clearInterval(timer); };
  }, [settings.captureSystemAudio, settings.selectedMicId, disabled]);

  const displays = sources.filter((s) => s.sourceType === "display");
  const selectedSource = displays.find((s) => s.id === settings.selectedSourceId);
  const selectedMic = mics.find((m) => m.id === settings.selectedMicId);
  const selectedCamera = cameras.find((c) => c.id === settings.selectedCameraId) ?? cameras[0];
  const screenOn = settings.selectedSourceId !== null;
  const micOn = settings.selectedMicId !== null;
  const cameraOn = settings.cameraBubble.enabled;
  const screenPermissionProblem = needsScreenPermission && permissions.screenRecording !== "authorized";
  const cameraPermissionProblem = cameraOn && permissions.camera !== "authorized";
  const micPermissionProblem = micOn && permissions.microphone !== "authorized";

  const toggleMenu = (menu: Menu) => setOpenDropdown(openDropdown === menu ? null : menu);

  const permissionHint = (kind: "screen" | "camera" | "microphone") => {
    const state = kind === "screen" ? permissions.screenRecording : permissions[kind];
    if (state === "denied" || state === "restricted") return `Permission blocked — open ${hostText.systemSettings}`;
    return "Permission required — activate to allow";
  };
  const issue = (kind: "screen" | "camera" | "microphone", id: string, onClick?: () => Promise<void>) => (
    <button id={id} type="button" onClick={() => void onClick?.()} className="source-permission-issue" aria-label={permissionHint(kind)}>
      {permissionHint(kind)}
    </button>
  );

  const turnScreen = (on: boolean) =>
    settings.setSelectedSourceId(on ? settings.availableSourceFallbackId ?? displays[0]?.id ?? null : null);
  const turnMic = (on: boolean) => {
    settings.setSelectedMicId(on ? (mics.find((m) => m.isDefault) ?? mics[0])?.id ?? null : null);
    if (on) void onRequestMicrophonePermission?.();
  };
  const turnCamera = (on: boolean) => {
    settings.updateCameraBubble({ enabled: on });
    if (on && !settings.selectedCameraId && cameras[0]) settings.setSelectedCameraId(cameras[0].id);
    if (on) void onRequestCameraPermission?.();
  };

  return (
    <div ref={containerRef} onKeyDown={handleMenuKeyDown} className="relative flex w-full flex-col gap-2 select-none">
      <SourceRow
        icon={<Monitor className="h-4 w-4" />}
        title="Screen"
        detail={
          screenOn && selectedSource
            ? `${selectedSource.name} · ${selectedSource.width}×${selectedSource.height}`
            : displays.length === 0
              ? "No display found"
              : "Off"
        }
        on={screenOn}
        onToggle={turnScreen}
        switchLabel="Record screen"
        switchDisabled={displays.length === 0}
        disabled={disabled}
        menu="source"
        open={openDropdown === "source"}
        onOpen={() => toggleMenu("source")}
        menuId="display-source-menu"
        issue={screenPermissionProblem && issue("screen", "screen-permission-status", onRequestScreenPermission)}
      >
        {openDropdown === "source" && (
          <MenuPanel id="display-source-menu" label="Displays">
            <div className="max-h-56 space-y-0.5 overflow-y-auto">
              {displays.length === 0 ? (
                <p className="px-2.5 py-3 text-center text-xs text-studio-400">No displays found</p>
              ) : (
                displays.map((display) => (
                  <MenuItem
                    key={display.id}
                    selected={settings.selectedSourceId === display.id}
                    onSelect={() => {
                      settings.setSelectedSourceId(display.id);
                      setOpenDropdown(null);
                    }}
                  >
                    {display.name}
                    <span className="ml-2 text-xs text-studio-500">
                      {display.width}×{display.height}
                    </span>
                  </MenuItem>
                ))
              )}
            </div>
          </MenuPanel>
        )}
      </SourceRow>

      <SourceRow
        icon={<Mic className="h-4 w-4" />}
        title="Microphone"
        detail={
          micOn && selectedMic ? (
            <>
              {selectedMic.name}
              {settings.micGainDb !== 0 && ` · ${settings.micGainDb > 0 ? "+" : ""}${settings.micGainDb} dB`}
              <LevelMeter peakDb={micPeakDb} label="microphone audio" />
            </>
          ) : mics.length === 0 ? (
            "No microphone found"
          ) : (
            "Off"
          )
        }
        on={micOn}
        onToggle={turnMic}
        switchLabel="Record microphone"
        switchDisabled={mics.length === 0}
        disabled={disabled}
        menu="mic"
        open={openDropdown === "mic"}
        onOpen={() => toggleMenu("mic")}
        menuId="microphone-source-menu"
        issue={micPermissionProblem && issue("microphone", "microphone-permission-status", onRequestMicrophonePermission)}
      >
        {openDropdown === "mic" && (
          <MenuPanel id="microphone-source-menu" label="Microphones">
            <div className="max-h-48 space-y-0.5 overflow-y-auto">
              {mics.length === 0 ? (
                <p className="px-2.5 py-3 text-center text-xs text-studio-400">No microphones found</p>
              ) : (
                mics.map((mic) => (
                  <MenuItem
                    key={mic.id}
                    selected={settings.selectedMicId === mic.id}
                    onSelect={() => {
                      settings.setSelectedMicId(mic.id);
                      setOpenDropdown(null);
                      void onRequestMicrophonePermission?.();
                    }}
                  >
                    {mic.name}
                  </MenuItem>
                ))
              )}
            </div>
            {selectedMic && (
              <div className="mt-1.5 space-y-2 border-t border-studio-800 px-2.5 pb-1.5 pt-2.5">
                <MicGainSlider value={settings.micGainDb} onChange={settings.setMicGainDb} peakDb={micPeakDb} disabled={disabled} />
                <p className="text-xs leading-snug text-studio-500">
                  Gain applies to the recorded mic track. Above 0 dB, loud input can clip.
                </p>
              </div>
            )}
          </MenuPanel>
        )}
      </SourceRow>

      <SourceRow
        icon={<Camera className="h-4 w-4" />}
        title="Camera"
        detail={cameraOn && selectedCamera ? selectedCamera.name : cameras.length === 0 ? "No camera found" : "Off"}
        on={cameraOn}
        onToggle={turnCamera}
        switchLabel="Record camera"
        switchDisabled={cameras.length === 0}
        disabled={disabled}
        menu="camera"
        open={openDropdown === "camera"}
        onOpen={() => toggleMenu("camera")}
        menuId="camera-source-menu"
        issue={cameraPermissionProblem && issue("camera", "camera-permission-status", onRequestCameraPermission)}
      >
        {openDropdown === "camera" && (
          <MenuPanel id="camera-source-menu" label="Cameras">
            <div className="max-h-56 space-y-0.5 overflow-y-auto">
              {cameras.length === 0 ? (
                <p className="px-2.5 py-3 text-center text-xs text-studio-400">No cameras found</p>
              ) : (
                cameras.map((camera) => (
                  <MenuItem
                    key={camera.id}
                    selected={cameraOn && settings.selectedCameraId === camera.id}
                    onSelect={() => {
                      settings.updateCameraBubble({ enabled: true });
                      settings.setSelectedCameraId(camera.id);
                      setOpenDropdown(null);
                      void onRequestCameraPermission?.();
                    }}
                  >
                    {camera.name}
                  </MenuItem>
                ))
              )}
            </div>
          </MenuPanel>
        )}
      </SourceRow>

      <SourceRow
        icon={<Volume2 className="h-4 w-4" />}
        title="Computer audio"
        detail={
          settings.captureSystemAudio ? (
            <>
              Apps, music, and calls
              <LevelMeter peakDb={systemPeakDb} label="computer audio" />
            </>
          ) : (
            "Off"
          )
        }
        on={settings.captureSystemAudio}
        onToggle={settings.setCaptureSystemAudio}
        switchLabel="Record computer audio"
        disabled={disabled}
      />
    </div>
  );
};
