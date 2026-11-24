import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";

export type MenuItem =
  | { label: string; onSelect: () => void; hint?: string; disabled?: boolean; danger?: boolean }
  | "separator";

/**
 * A right-click menu at the pointer. Arrow keys move, Enter picks, Escape or a
 * click elsewhere closes it; it stays inside the window. Closing it from the
 * keyboard, or picking an item, puts focus back on `returnFocus()` (or whatever
 * had focus when it opened) before the item runs, so an item that opens
 * something else still decides where focus ends up.
 */
export default function ContextMenu({
  x,
  y,
  items,
  onClose,
  label = "Actions",
  returnFocus,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
  label?: string;
  returnFocus?: () => HTMLElement | null;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const opener = useRef(document.activeElement);
  const restore = () => {
    const target = returnFocus?.() ?? opener.current;
    if (target instanceof HTMLElement && target.isConnected) target.focus({ preventScroll: true });
  };
  const [place, setPlace] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const box = ref.current?.getBoundingClientRect();
    if (!box) return;
    setPlace({
      left: Math.max(4, Math.min(x, window.innerWidth - box.width - 4)),
      top: Math.max(4, Math.min(y, window.innerHeight - box.height - 4)),
    });
    ref.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [x, y]);

  useEffect(() => {
    const away = (event: MouseEvent) => {
      if (!ref.current?.contains(event.target as Node)) onClose();
    };
    const blur = () => onClose();
    window.addEventListener("mousedown", away, true);
    window.addEventListener("blur", blur);
    window.addEventListener("resize", blur);
    return () => {
      window.removeEventListener("mousedown", away, true);
      window.removeEventListener("blur", blur);
      window.removeEventListener("resize", blur);
    };
  }, [onClose]);

  const onKeyDown = (event: KeyboardEvent) => {
    const buttons = [...(ref.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      restore();
      onClose();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      buttons[(at + step + buttons.length) % buttons.length]?.focus();
    }
  };

  return createPortal(
    <div ref={ref} className="ctx-menu" role="menu" aria-label={label} style={place} onKeyDown={onKeyDown}>
      {items.map((item, index) =>
        item === "separator" ? (
          <hr key={`sep-${index}`} />
        ) : (
          <button
            type="button"
            role="menuitem"
            key={item.label}
            disabled={item.disabled}
            className={item.danger ? "danger" : undefined}
            onClick={() => {
              restore();
              onClose();
              item.onSelect();
            }}
          >
            <span>{item.label}</span>
            {item.hint ? <kbd>{item.hint}</kbd> : null}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}
