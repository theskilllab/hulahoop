/**
 * Theme handling.
 *
 * The theme is personal and lives in the browser, like the prototype: each one
 * is either light or dark, and "Match system" follows the Mac. A theme is a
 * palette of tokens in `styles.css`, so switching two attributes on `<html>`
 * repaints everything without a re-render: `data-theme` carries the resolved
 * light-or-dark, `data-palette` picks the colour set.
 */

export interface ThemeChoice {
  id: string;
  group: string;
  name: string;
  kind: "light" | "dark" | "auto";
  blurb: string;
  /** Six swatch dots: surface, raised surface, text, accent, ok, bad. */
  swatch: string[];
  /** The auto theme shows the swatch of whichever side the system picked. */
  darkSwatch?: string[];
}

const DEFAULT_LIGHT = [
  "#ffffff",
  "#f6f6f7",
  "#17171a",
  "#0a6cff",
  "#15803d",
  "#b42318",
];
const DEFAULT_DARK = [
  "#121214",
  "#1a1a1d",
  "#f3f3f5",
  "#1469f0",
  "#5fd18a",
  "#ff8a80",
];

export const THEMES: ThemeChoice[] = [
  {
    id: "default-light",
    group: "Default",
    name: "Light",
    kind: "light",
    blurb: "Neutral greys with system blue.",
    swatch: DEFAULT_LIGHT,
  },
  {
    id: "default-dark",
    group: "Default",
    name: "Dark",
    kind: "dark",
    blurb: "The original dark. Near-black greys with system blue.",
    swatch: DEFAULT_DARK,
  },
  {
    id: "default",
    group: "Default",
    name: "Match system",
    kind: "auto",
    blurb: "Switches between Default Light and Default Dark with your Mac.",
    swatch: DEFAULT_LIGHT,
    darkSwatch: DEFAULT_DARK,
  },
  {
    id: "rose-dawn",
    group: "Rosé Pine",
    name: "Dawn",
    kind: "light",
    blurb: "The light one. Warm paper with pine and rose.",
    swatch: ["#faf4ed", "#f4ede8", "#575279", "#286983", "#2f6d78", "#a3475f"],
  },
  {
    id: "rose",
    group: "Rosé Pine",
    name: "Rosé Pine",
    kind: "dark",
    blurb: "The original dark. Deep purple-black.",
    swatch: ["#191724", "#1f1d2e", "#e0def4", "#2f7e9c", "#9ccfd8", "#eb6f92"],
  },
  {
    id: "rose-moon",
    group: "Rosé Pine",
    name: "Moon",
    kind: "dark",
    blurb: "A little lighter and bluer than the original.",
    swatch: ["#232136", "#2a273f", "#e0def4", "#31748f", "#9ccfd8", "#eb6f92"],
  },
  {
    id: "latte",
    group: "Catppuccin",
    name: "Latte",
    kind: "light",
    blurb: "The light flavour.",
    swatch: ["#eff1f5", "#e6e9ef", "#4c4f69", "#1e66f5", "#2e7d1c", "#d20f39"],
  },
  {
    id: "frappe",
    group: "Catppuccin",
    name: "Frappé",
    kind: "dark",
    blurb: "Dark with a soft blue-grey base.",
    swatch: ["#303446", "#292c3c", "#c6d0f5", "#8caaee", "#a6d189", "#e78284"],
  },
  {
    id: "macchiato",
    group: "Catppuccin",
    name: "Macchiato",
    kind: "dark",
    blurb: "Darker and deeper than Frappé.",
    swatch: ["#24273a", "#1e2030", "#cad3f5", "#8aadf4", "#a6da95", "#ed8796"],
  },
  {
    id: "mocha",
    group: "Catppuccin",
    name: "Mocha",
    kind: "dark",
    blurb: "The darkest flavour.",
    swatch: ["#1e1e2e", "#181825", "#cdd6f4", "#89b4fa", "#a6e3a1", "#f38ba8"],
  },
];

export const THEME_GROUPS = ["Default", "Rosé Pine", "Catppuccin"];

const KEY = "hh-palette";
const LEGACY_KEY = "hh-theme";

export function themeOf(id: string): ThemeChoice {
  return THEMES.find((theme) => theme.id === id) ?? THEMES[2];
}

/** The saved choice, carrying the old light/dark/system setting across. */
export function savedPalette(): string {
  const value = localStorage.getItem(KEY);
  if (value && THEMES.some((theme) => theme.id === value)) return value;
  const legacy = localStorage.getItem(LEGACY_KEY);
  if (legacy === "light") return "default-light";
  if (legacy === "dark") return "default-dark";
  return "default";
}

function systemDark(): boolean {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches === true;
}

/** What the page is actually painted as right now. */
export function resolvedTheme(id: string = savedPalette()): "light" | "dark" {
  const theme = themeOf(id);
  return theme.kind === "auto" ? (systemDark() ? "dark" : "light") : theme.kind;
}

export function applyTheme(id: string = savedPalette()): void {
  const theme = themeOf(id);
  localStorage.setItem(KEY, theme.id);
  const root = document.documentElement;
  root.dataset.theme = resolvedTheme(theme.id);
  root.dataset.palette = theme.id.startsWith("default") ? "default" : theme.id;
}

export function watchSystemTheme(): () => void {
  const media = window.matchMedia?.("(prefers-color-scheme: dark)");
  if (!media) return () => {};
  const listener = () => {
    if (themeOf(savedPalette()).kind === "auto") applyTheme();
  };
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
}
