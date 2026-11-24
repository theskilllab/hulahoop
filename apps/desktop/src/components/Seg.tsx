import type { ReactNode } from "react";

export interface SegOption<T extends string> {
  value: T;
  label: ReactNode;
  /** Accessible name when the label is only an icon. */
  title?: string;
}

/** The segmented control from DESIGN.md: one pressed plate, `aria-pressed` on each button. */
export default function Seg<T extends string>({
  value,
  options,
  onChange,
  label,
  small = false,
}: {
  value: T;
  options: SegOption<T>[];
  onChange: (value: T) => void;
  /** The group's accessible name. */
  label: string;
  small?: boolean;
}) {
  return (
    <div className={small ? "seg small" : "seg"} role="group" aria-label={label}>
      {options.map((option) => (
        <button
          type="button"
          key={option.value}
          aria-pressed={option.value === value}
          title={option.title}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
