import { COUNTDOWN_SECONDS, useSettingsStore } from "../../stores/settingsStore";
import { Select, SettingRow } from "../ui/controls";

export function CountdownControl({ disabled }: { disabled: boolean }) {
  const { countdownSeconds, setCountdownSeconds } = useSettingsStore();

  return (
    <SettingRow title="Countdown" description="Time to get ready. Every source warms up meanwhile, so all tracks start together.">
      <Select<number>
        ariaLabel="Countdown"
        className="w-36"
        value={countdownSeconds}
        options={COUNTDOWN_SECONDS}
        format={(seconds) => (seconds === 0 ? "Off" : `${seconds} seconds`)}
        onChange={setCountdownSeconds}
        disabled={disabled}
      />
    </SettingRow>
  );
}
