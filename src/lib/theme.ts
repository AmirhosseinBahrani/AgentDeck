import { useEffect, useState } from "react";

export type Theme = "light" | "dark";

const KEY = "agentdeck.theme";

/**
 * Reads the theme before React mounts.
 *
 * A stored choice wins over the system preference, because someone who picked light on a machine
 * set to dark meant it. With neither, the OS decides — a desktop app that ignores that is the
 * one window on screen that looks wrong.
 */
export function initialTheme(): Theme {
  const stored = localStorage.getItem(KEY);
  if (stored === "light" || stored === "dark") return stored;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/** Applied to `<html>` rather than a wrapper so dialogs and portals inherit it too. */
export function applyTheme(theme: Theme): void {
  document.documentElement.classList.toggle("dark", theme === "dark");
}

export function useTheme(): [Theme, (next: Theme) => void] {
  const [theme, setThemeState] = useState<Theme>(initialTheme);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  // Only while the operator has expressed no preference of their own. Following the system after
  // someone has chosen would silently overrule them.
  useEffect(() => {
    if (localStorage.getItem(KEY)) return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (e: MediaQueryListEvent) => setThemeState(e.matches ? "dark" : "light");
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, []);

  return [
    theme,
    (next: Theme) => {
      localStorage.setItem(KEY, next);
      setThemeState(next);
    },
  ];
}
