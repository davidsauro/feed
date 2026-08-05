<script setup lang="ts">
/**
 * Three-way theme switch for the sidebar footer.
 *
 * "System" is kept as an option, but it can't be trusted under WSL2 — see the
 * note in theme.ts — which is why the manual choices exist.
 */
import type { ThemeMode } from "../theme";
import { setThemeMode, themeMode } from "../theme";

const OPTIONS: { mode: ThemeMode; title: string }[] = [
  { mode: "system", title: "Match the desktop theme" },
  { mode: "light", title: "Light theme" },
  { mode: "dark", title: "Dark theme" },
];
</script>

<template>
  <div class="switch" role="group" aria-label="Theme">
    <button
      v-for="option in OPTIONS"
      :key="option.mode"
      class="option"
      :class="{ active: themeMode === option.mode }"
      :title="option.title"
      :aria-pressed="themeMode === option.mode"
      @click="setThemeMode(option.mode)"
    >
      <!-- Monitor: follow the desktop. -->
      <svg
        v-if="option.mode === 'system'"
        viewBox="0 0 24 24"
        width="13"
        height="13"
        stroke="currentColor"
        stroke-width="2"
        fill="none"
      >
        <rect x="2" y="3" width="20" height="14" rx="2" />
        <line x1="8" y1="21" x2="16" y2="21" />
      </svg>

      <!-- Sun: light. -->
      <svg
        v-else-if="option.mode === 'light'"
        viewBox="0 0 24 24"
        width="13"
        height="13"
        stroke="currentColor"
        stroke-width="2"
        fill="none"
      >
        <circle cx="12" cy="12" r="4.5" />
        <path
          d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4"
        />
      </svg>

      <!-- Moon: dark. -->
      <svg
        v-else
        viewBox="0 0 24 24"
        width="13"
        height="13"
        stroke="currentColor"
        stroke-width="2"
        fill="none"
      >
        <path d="M21 12.8A8.5 8.5 0 1 1 11.2 3a6.6 6.6 0 0 0 9.8 9.8z" />
      </svg>
    </button>
  </div>
</template>

<style scoped>
.switch {
  display: flex;
  gap: 1px;
  padding: 2px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background-color: var(--bg);
}

.option {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 18px;
  border-radius: 4px;
  color: var(--text-faint);
}

.option:hover {
  color: var(--text-muted);
}

.option.active {
  background-color: var(--bg-active);
  color: var(--accent);
}
</style>
