/**
 * Light and dark theme handling.
 *
 * The stylesheet does not use `prefers-color-scheme` at all. Instead this module
 * resolves the user's choice down to a concrete theme and writes it to
 * `<html data-theme="light|dark">`, which is the only thing styles.css looks at.
 *
 * The reason is WSL2: the webview there is webkit2gtk, so `prefers-color-scheme`
 * reports the GTK theme inside the Linux distro rather than the Windows setting,
 * and answers "light" even when Windows is in dark mode. Resolving in one place
 * means "System" can be wrong without the manual choices being affected.
 */
import { ref } from "vue";

/** What the user picked. "system" follows the desktop; the others are absolute. */
export type ThemeMode = "system" | "light" | "dark";

/** What actually gets applied to the document. */
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "feed.theme-mode";

const systemPrefersDark = window.matchMedia("(prefers-color-scheme: dark)");

/** The current choice. Read it to render UI; change it with `setThemeMode`. */
export const themeMode = ref<ThemeMode>("system");

function isThemeMode(value: string | null): value is ThemeMode {
  return value === "system" || value === "light" || value === "dark";
}

/** Turns a choice into the theme to actually apply. */
export function resolveTheme(mode: ThemeMode): ResolvedTheme {
  if (mode === "system") {
    return systemPrefersDark.matches ? "dark" : "light";
  }

  return mode;
}

function applyTheme(mode: ThemeMode) {
  document.documentElement.dataset.theme = resolveTheme(mode);
}

/** Changes the theme and remembers it for next launch. */
export function setThemeMode(mode: ThemeMode) {
  themeMode.value = mode;
  applyTheme(mode);

  try {
    window.localStorage.setItem(STORAGE_KEY, mode);
  } catch (error) {
    // Not worth surfacing: the theme still applies, it just won't persist.
    console.error("Could not save the theme choice", error);
  }
}

/**
 * Loads the saved theme and applies it.
 *
 * Call this before mounting the app so the window opens in the right theme
 * instead of starting light and correcting itself.
 */
export function initTheme() {
  let saved: string | null = null;
  try {
    saved = window.localStorage.getItem(STORAGE_KEY);
  } catch {
    // Private mode or a locked-down webview. Fall through to the default.
  }

  themeMode.value = isThemeMode(saved) ? saved : "system";
  applyTheme(themeMode.value);

  // Only matters while "System" is selected; an explicit choice ignores it.
  systemPrefersDark.addEventListener("change", () => {
    if (themeMode.value === "system") {
      applyTheme("system");
    }
  });
}
