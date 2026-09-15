import { COUNTDOWN_SECONDS, useSettingsStore } from "../../stores/settingsStore";
import { Segmented } from "./RecordingQualityControl";

export function CountdownControl({ disabled }: { disabled: boolean }) {
  const { countdownSeconds, setCountdownSeconds } = useSettingsStore();

  return (
    <div className="space-y-1.5">
      <Segmented<number>
        label="Before recording"
        options={COUNTDOWN_SECONDS}
        value={countdownSeconds}
        onChange={setCountdownSeconds}
        format={(seconds) => (seconds === 0 ? "Off" : `${seconds}s`)}
        disabled={disabled}
      />
      <p className="text-[10px] leading-snug text-studio-500">
        The camera and microphone warm up during the countdown, so every track starts together.
      </p>
    </div>
  );
}
