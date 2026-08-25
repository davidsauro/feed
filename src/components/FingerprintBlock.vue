<script setup lang="ts">
/**
 * A fingerprint, laid out to be compared against another screen.
 *
 * One component because the same code is shown while adding somebody, in
 * Settings, and on tapping your own name, and two people comparing two screens
 * should not have to allow for the two looking different. Monospace so the
 * groups line up, and spaced so the eye can hold four characters at a time.
 */
defineProps<{
  label: string;
  /** The code in groups, or empty while it is being worked out. */
  code: string;
}>();
</script>

<template>
  <div class="block">
    <span class="label">{{ label }}</span>
    <code class="code" :class="{ pending: !code }">{{ code || "working it out…" }}</code>
  </div>
</template>

<style scoped>
.block {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 12px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  background-color: var(--bg-sunken);
}

.label {
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: var(--text-faint);
}

.code {
  font-family: var(--font-mono);
  font-size: 15px;
  font-weight: 600;
  letter-spacing: 0.04em;
  line-height: 1.6;
  color: var(--text);
  /* Wraps a whole group at a time rather than splitting one, so the four
     character rhythm survives a narrow window. */
  overflow-wrap: break-word;
}

.code.pending {
  font-size: 13px;
  font-weight: 400;
  color: var(--text-faint);
}
</style>
