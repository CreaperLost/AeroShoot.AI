import React, { useId } from "react";
import { ChevronDown } from "lucide-react";

/** On/off switch. `label` names it for assistive technology. */
export function Switch({
  checked,
  onChange,
  label,
  disabled = false,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative h-5 w-9 shrink-0 rounded-full transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-400 disabled:cursor-not-allowed disabled:opacity-40 ${
        checked ? "bg-indigo-500" : "bg-studio-600"
      }`}
    >
      <span
        aria-hidden="true"
        className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all ${checked ? "left-[18px]" : "left-0.5"}`}
      />
    </button>
  );
}

/** A native dropdown with a visible label above it. */
export function SelectField<T extends string | number>({
  label,
  value,
  options,
  format,
  onChange,
  disabled = false,
}: {
  label: string;
  value: T;
  options: readonly T[];
  format: (value: T) => string;
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  const id = useId();
  return (
    <div className="min-w-0">
      <label htmlFor={id} className="mb-1 block text-xs text-studio-400">
        {label}
      </label>
      <Select
        id={id}
        value={value}
        options={options}
        format={format}
        onChange={onChange}
        disabled={disabled}
      />
    </div>
  );
}

export function Select<T extends string | number>({
  id,
  value,
  options,
  format,
  onChange,
  disabled = false,
  ariaLabel,
  className = "w-full",
}: {
  id?: string;
  value: T;
  options: readonly T[];
  format: (value: T) => string;
  onChange: (value: T) => void;
  disabled?: boolean;
  ariaLabel?: string;
  className?: string;
}) {
  return (
    <div className={`relative ${className}`}>
      <select
        id={id}
        aria-label={ariaLabel}
        value={String(value)}
        disabled={disabled}
        onChange={(event) => {
          const next = options.find((option) => String(option) === event.target.value);
          if (next !== undefined) onChange(next);
        }}
        className="w-full appearance-none rounded-lg border border-studio-700 bg-studio-950 py-1.5 pl-2.5 pr-7 text-[13px] text-white focus:border-indigo-500 focus:outline-none disabled:opacity-50"
      >
        {options.map((option) => (
          <option key={String(option)} value={String(option)}>
            {format(option)}
          </option>
        ))}
      </select>
      <ChevronDown
        aria-hidden="true"
        className="pointer-events-none absolute right-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-studio-400"
      />
    </div>
  );
}

/** One setting per row: name and description on the left, control on the right. */
export function SettingRow({
  title,
  description,
  children,
}: {
  title: string;
  description?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex min-h-11 items-center gap-4 py-2 [&+&]:border-t [&+&]:border-studio-800">
      <div className="min-w-0 flex-1">
        <div className="text-[13px] text-white">{title}</div>
        {description && <div className="mt-0.5 text-xs leading-snug text-studio-400">{description}</div>}
      </div>
      <div className="flex shrink-0 items-center">{children}</div>
    </div>
  );
}

/** A titled group in the settings dialog. */
export function SettingsSection({
  title,
  icon,
  children,
}: {
  title: string;
  icon: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section aria-label={title} className="border-t border-studio-800 pt-4 first:border-t-0 first:pt-0">
      <h3 className="mb-3 flex items-center gap-2 text-sm font-medium text-white">
        <span aria-hidden="true" className="text-studio-400">
          {icon}
        </span>
        {title}
      </h3>
      {children}
    </section>
  );
}
