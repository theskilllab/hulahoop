import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import Icon from "../Icon";

export interface SelectOption {
  value: string;
  label: string;
  /** Options with the same group sit under one small heading. */
  group?: string;
  disabled?: boolean;
  /** Muted text on the right of the row: a price, or why the option is disabled. */
  detail?: string;
}

const SEARCH_ABOVE = 4;
const MIN_WIDTH = 200;
const GAP = 4;

/**
 * The shared dropdown: a pill that opens a `.sel-pop` listbox under it. Above four
 * options it gains a search row; without one, typing a letter jumps to the next
 * option starting with it. Inside a `<dialog>` the list renders inside that dialog,
 * so it stays above the modal backdrop.
 */
export default function Select({
  label,
  value,
  options,
  onChange,
  prefix,
  id,
  disabled,
  title,
  searchable,
}: {
  label: string;
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  /** Shown before the current choice on the pill, e.g. a colour dot. */
  prefix?: ReactNode;
  id?: string;
  /** A disabled pill cannot open; `title` says why. */
  disabled?: boolean;
  title?: string;
  /** Force the search row on or off; by default it shows above four options. */
  searchable?: boolean;
}) {
  const trigger = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const current = options.find((option) => option.value === value);

  const close = (refocus: boolean) => {
    setOpen(false);
    if (refocus) trigger.current?.focus();
  };

  return (
    <span className="sel" onKeyDown={(event) => !open && openOnArrow(event, () => setOpen(true))}>
      <button
        ref={trigger}
        id={id}
        type="button"
        disabled={disabled}
        className="pill sel-btn"
        aria-label={label}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={title}
        onClick={() => setOpen((was) => !was)}
      >
        {prefix}
        <span className="sel-value">{current?.label ?? ""}</span>
        <Icon name="chevdown" size={12} />
      </button>
      {open ? (
        <Popover
          trigger={trigger.current as HTMLButtonElement}
          label={label}
          value={value}
          options={options}
          searchable={searchable ?? options.length > SEARCH_ABOVE}
          onChoose={(next) => {
            if (next !== value) onChange(next);
            close(true);
          }}
          onClose={close}
        />
      ) : null}
    </span>
  );
}

function openOnArrow(event: KeyboardEvent, open: () => void) {
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  event.preventDefault();
  open();
}

function Popover({
  trigger,
  label,
  value,
  options,
  searchable,
  onChoose,
  onClose,
}: {
  trigger: HTMLButtonElement;
  label: string;
  value: string;
  options: SelectOption[];
  searchable: boolean;
  onChoose: (value: string) => void;
  onClose: (refocus: boolean) => void;
}) {
  const pop = useRef<HTMLDivElement>(null);
  const searchBox = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return needle ? options.filter((option) => `${option.label} ${option.group ?? ""}`.toLowerCase().includes(needle)) : options;
  }, [options, query]);
  const [active, setActive] = useState(() => Math.max(0, options.findIndex((option) => option.value === value && !option.disabled)));
  const [place, setPlace] = useState<{ left: number; top: number; minWidth: number }>(() => anchor(trigger));
  const host = trigger.closest("dialog") ?? document.body;

  useLayoutEffect(() => {
    setPlace(fit(trigger, pop.current));
  }, [trigger, shown.length]);

  useEffect(() => {
    (searchBox.current ?? pop.current)?.focus();
    const outside = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!pop.current?.contains(target) && !trigger.contains(target)) onClose(false);
    };
    document.addEventListener("mousedown", outside);
    return () => document.removeEventListener("mousedown", outside);
  }, [trigger, onClose]);

  useEffect(() => {
    pop.current?.querySelector(".active")?.scrollIntoView?.({ block: "nearest" });
  }, [active, shown]);

  const step = (delta: number) => {
    const enabled = shown.map((option, index) => (option.disabled ? -1 : index)).filter((index) => index >= 0);
    if (!enabled.length) return;
    const at = enabled.indexOf(active);
    setActive(enabled[Math.min(Math.max(at + delta, 0), enabled.length - 1)]);
  };

  const jumpTo = (letter: string) => {
    const lower = letter.toLowerCase();
    for (let offset = 1; offset <= shown.length; offset += 1) {
      const index = (active + offset) % shown.length;
      if (!shown[index].disabled && shown[index].label.toLowerCase().startsWith(lower)) return setActive(index);
    }
  };

  function onKeyDown(event: KeyboardEvent) {
    const handled = ["ArrowDown", "ArrowUp", "Enter", "Escape"].includes(event.key);
    if (event.key === "Tab") return onClose(false);
    if (event.key === "ArrowDown") step(1);
    else if (event.key === "ArrowUp") step(-1);
    else if (event.key === "Escape") {
      event.stopPropagation();
      onClose(true);
    } else if (event.key === "Enter") {
      const option = shown[active];
      if (option && !option.disabled) onChoose(option.value);
    } else if (!searchable && event.key.length === 1 && event.key !== " " && !event.metaKey && !event.ctrlKey) jumpTo(event.key);
    if (handled || (event.key === " " && !searchable)) event.preventDefault();
  }

  let lastGroup: string | undefined;
  const rows = shown.map((option, index) => {
    const heading = option.group && option.group !== lastGroup ? <div className="sp-group" key={`g-${option.group}`}>{option.group}</div> : null;
    lastGroup = option.group;
    return [
      heading,
      <div
        key={option.value}
        role="option"
        aria-selected={option.value === value}
        aria-disabled={option.disabled || undefined}
        className={`sp-opt${index === active ? " active" : ""}${option.value === value ? " cur" : ""}${option.disabled ? " dis" : ""}`}
        onMouseMove={() => !option.disabled && setActive(index)}
        onClick={() => !option.disabled && onChoose(option.value)}
      >
        <span className="sp-label">{option.label}</span>
        {option.detail ? <span className="sp-detail">{option.detail}</span> : null}
        {option.value === value ? <Icon name="check" size={14} /> : null}
      </div>,
    ];
  });

  return createPortal(
    <div ref={pop} className="sel-pop" style={place} tabIndex={-1} onKeyDown={onKeyDown}>
      {searchable ? (
        <div className="sp-search">
          <input
            ref={searchBox}
            type="text"
            placeholder="Search…"
            aria-label={`Search ${label}`}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setActive(0);
            }}
          />
        </div>
      ) : null}
      <div role="listbox" aria-label={label} className="sp-list">
        {rows}
        {shown.length === 0 ? <div className="sp-empty">No matches</div> : null}
      </div>
    </div>,
    host,
  );
}

function anchor(trigger: HTMLElement) {
  const box = trigger.getBoundingClientRect();
  return { left: box.left, top: box.bottom + GAP, minWidth: Math.max(MIN_WIDTH, box.width) };
}

/** Below the control; above it when the window ends first; shifted left when it would overflow. */
function fit(trigger: HTMLElement, pop: HTMLElement | null) {
  const place = anchor(trigger);
  if (!pop) return place;
  const box = trigger.getBoundingClientRect();
  const size = pop.getBoundingClientRect();
  if (place.top + size.height > window.innerHeight && box.top - GAP - size.height >= 0) place.top = box.top - GAP - size.height;
  if (place.left + size.width > window.innerWidth) place.left = Math.max(GAP, window.innerWidth - size.width - GAP);
  return place;
}
