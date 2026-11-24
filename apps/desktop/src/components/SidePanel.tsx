import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import Icon from "../Icon";

/** Whether a press landed on its target's own scrollbar, outside its content box. */
function onScrollbar(event: MouseEvent): boolean {
  const target = event.target;
  if (!(target instanceof Element)) return false;
  if (target.scrollHeight <= target.clientHeight && target.scrollWidth <= target.clientWidth) return false;
  const rect = target.getBoundingClientRect();
  const x = event.clientX - rect.left - target.clientLeft;
  const y = event.clientY - rect.top - target.clientTop;
  return x >= target.clientWidth || y >= target.clientHeight || x < 0 || y < 0;
}

/**
 * A slide-over from the right edge of the work area: 80% wide, or the whole
 * area when maximized. It sits over the screen without a backdrop, so the list
 * behind stays visible and clickable.
 *
 * Escape peels one layer at a time: `onEscape` gets the first chance (a nested
 * view such as a transcript returns `true` to keep the panel), then a maximized
 * panel shrinks back, then the panel closes. Focus moves into the panel when it
 * opens and back to `returnFocus()` (or whatever had focus) when it closes.
 *
 * A click on the area the panel covers, outside the panel, puts it away the
 * same way: a maximized panel shrinks back first, then the panel closes. Only
 * a click that was pressed and released on the same element counts, and a
 * press on a scrollbar never does. Closed this way, focus stays where the
 * click put it. A click on another task (`[data-panel-opener]`) opens that
 * task instead.
 *
 * The parent must be `position: relative`; the panel covers it.
 */
export default function SidePanel({
  label,
  title,
  actions,
  onClose,
  onEscape,
  returnFocus,
  children,
}: {
  /** The panel's accessible name. */
  label: string;
  /** Left of the header, e.g. a task key. */
  title: ReactNode;
  /** Extra header buttons, left of maximize and close. */
  actions?: ReactNode;
  onClose: () => void;
  /** Return `true` when Escape was handled by an inner layer. */
  onEscape?: () => boolean;
  /** Where focus goes back on close; defaults to the element focused at open. */
  returnFocus?: () => HTMLElement | null;
  children: ReactNode;
}) {
  const ref = useRef<HTMLElement>(null);
  const [maximized, setMaximized] = useState(false);
  const opener = useRef<Element | null>(null);
  const escape = useRef({ onEscape, onClose, maximized });
  escape.current = { onEscape, onClose, maximized };
  const restore = useRef(returnFocus);
  restore.current = returnFocus;

  useLayoutEffect(() => {
    opener.current = document.activeElement;
    ref.current?.focus({ preventScroll: true });
    return () => {
      const target = restore.current?.() ?? opener.current;
      if (target instanceof HTMLElement && target.isConnected) target.focus({ preventScroll: true });
    };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      // A modal dialog or a menu owns Escape while it is open.
      if (document.querySelector("dialog[open], .ctx-menu, .palette")) return;
      const target = event.target as Node | null;
      const inside = target && ref.current?.contains(target);
      if (!inside && target !== document.body && !(target instanceof HTMLElement && target.closest("[data-panel-opener]"))) return;
      event.preventDefault();
      // The first Escape in a field only leaves the field, so a half-written
      // comment or answer is never closed away by accident.
      if (inside && target instanceof HTMLElement && target.closest("input, textarea, select, [contenteditable='true']")) {
        target.blur();
        ref.current?.focus({ preventScroll: true });
        return;
      }
      const layer = escape.current;
      if (layer.onEscape?.()) return;
      if (layer.maximized) {
        setMaximized(false);
        return;
      }
      layer.onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    // The element the primary button went down on, unless that was a scrollbar.
    let pressed: EventTarget | null = null;
    const onPointerDown = (event: PointerEvent) => {
      pressed = event.button === 0 && !onScrollbar(event) ? event.target : null;
    };
    const onClick = (event: MouseEvent) => {
      const start = pressed;
      pressed = null;
      if (event.button !== 0 || event.target !== start) return;
      const panel = ref.current;
      const target = event.target;
      if (!panel || !(target instanceof Element) || panel.contains(target)) return;
      // Only the area the panel slides over counts; the rail, the page bar and
      // portalled menus are elsewhere in the document.
      if (!panel.parentElement?.contains(target)) return;
      // A task row beside the panel opens that task in its place.
      if (target.closest("[data-panel-opener]")) return;
      if (document.querySelector("dialog[open], .ctx-menu, .palette")) return;
      // Anywhere else in the strip beside the panel is its backdrop: a click there
      // closes the panel exactly like its close button.
      escape.current.onClose();
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("click", onClick);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("click", onClick);
    };
  }, []);

  return (
    <aside
      ref={ref}
      className={maximized ? "side-panel max" : "side-panel"}
      aria-label={label}
      role="dialog"
      aria-modal="false"
      tabIndex={-1}
    >
      <header className="side-panel-head">
        <div className="side-panel-title">{title}</div>
        {actions}
        <button
          type="button"
          className="icon-btn"
          aria-label={maximized ? "Restore panel size" : "Maximize panel"}
          aria-pressed={maximized}
          title={maximized ? "Restore" : "Maximize"}
          onClick={() => setMaximized((value) => !value)}
        >
          <Icon name={maximized ? "shrink" : "expand"} size={14} />
        </button>
        <button
          type="button"
          className="icon-btn"
          aria-label="Close (Esc)"
          title="Close (Esc)"
          onClick={onClose}
        >
          <Icon name="close" size={14} />
        </button>
      </header>
      <div className="side-panel-body">{children}</div>
    </aside>
  );
}
