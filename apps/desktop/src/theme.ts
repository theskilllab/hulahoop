/**
 * Theme handling.
 *
 * The palette is personal and lives in the browser, like the prototype: `system`
 * follows the Mac, and the two explicit choices pin light or dark. Every colour
 * comes from the tokens in `styles.css`, so switching the attribute on `<html>`
 * repaints everything without a re-render.
 */
export type Theme = "system" | "light" | "dark";

const KEY = "hh-theme";

export function savedTheme(): Theme {
  const value = localStorage.getItem(KEY);
  return value === "light" || value === "dark" ? value : "system";
}

export function applyTheme(theme: Theme = savedTheme()): void {
  localStorage.setItem(KEY, theme);
  const dark =
    theme === "dark" ||
    (theme === "system" && window.matchMedia?.("(prefers-color-scheme: dark)").matches === true);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

export function watchSystemTheme(): () => void {
  const media = window.matchMedia?.("(prefers-color-scheme: dark)");
  if (!media) return () => {};
  const listener = () => {
    if (savedTheme() === "system") applyTheme("system");
  };
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
}
