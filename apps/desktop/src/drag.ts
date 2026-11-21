import { invoke } from "./api";

/// The window has an overlay title bar, so a person drags it by its own furniture:
/// the brand row and every page header. A DOM attribute only covers the element it
/// sits on, and those rows are full of children, so the move is asked for on mousedown.
export function installDrag(): void {
  document.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (!target?.closest("[data-drag]")) return;
    if (target.closest("button, a, input, select, textarea, [role='menu']")) return;
    void invoke("start_drag").catch(() => undefined);
  });
}
