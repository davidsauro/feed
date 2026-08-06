<script setup lang="ts">
/**
 * Sidebar header showing this node's own peer ID.
 *
 * The full ID is 52 characters and is only ever needed to hand to someone else,
 * so it's shortened here with a button to copy the real value.
 */
import { ref } from "vue";
import { shortPeerId } from "../types";

const props = defineProps<{
  peerId: string;
  /** The name chosen in Settings, which other nodes are told. Empty if unset. */
  name: string;
}>();

const justCopied = ref(false);

async function copyPeerId() {
  try {
    await navigator.clipboard.writeText(props.peerId);
  } catch {
    // Nothing useful to do if the clipboard is unavailable; the ID is still
    // selectable by hand.
    return;
  }

  justCopied.value = true;
  window.setTimeout(() => {
    justCopied.value = false;
  }, 1500);
}
</script>

<template>
  <header class="identity">
    <span class="label">My node</span>

    <div class="row">
      <span class="text">
        <span v-if="name" class="name">{{ name }}</span>

        <!-- Drops to a subtitle once there's a name above it, since the id is
             then only needed for reading out or copying. -->
        <code class="peer-id" :class="{ secondary: name }" :title="peerId">
          {{ peerId ? shortPeerId(peerId) : "…" }}
        </code>
      </span>

      <button
        class="copy"
        :disabled="!peerId"
        :title="justCopied ? 'Copied' : 'Copy full peer ID'"
        @click="copyPeerId"
      >
        <!-- Checkmark once copied, two overlapping pages otherwise. -->
        <svg
          v-if="justCopied"
          viewBox="0 0 24 24"
          width="14"
          height="14"
          stroke="currentColor"
          stroke-width="2.5"
          fill="none"
        >
          <polyline points="20 6 9 17 4 12" />
        </svg>

        <svg
          v-else
          viewBox="0 0 24 24"
          width="14"
          height="14"
          stroke="currentColor"
          stroke-width="2"
          fill="none"
        >
          <rect x="9" y="9" width="11" height="11" rx="2" />
          <path d="M5 15V5a2 2 0 0 1 2-2h8" />
        </svg>
      </button>
    </div>
  </header>
</template>

<style scoped>
.identity {
  padding: 14px 14px 12px;
  border-bottom: 1px solid var(--border);
}

.label {
  display: block;
  margin-bottom: 4px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--text-faint);
}

.row {
  display: flex;
  align-items: center;
  gap: 6px;
}

.text {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
}

.name {
  overflow: hidden;
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.peer-id {
  overflow: hidden;
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--text);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.peer-id.secondary {
  font-size: 10px;
  color: var(--text-faint);
}

.copy {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  color: var(--text-faint);
}

.copy:hover:not(:disabled) {
  background-color: var(--bg-hover);
  color: var(--text);
}
</style>
