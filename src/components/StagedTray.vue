<script setup lang="ts">
/**
 * Files picked but not sent, waiting under whoever they are going to.
 *
 * Picking and sending are two steps on purpose: a batch collects here, where it
 * can be looked over and pruned, and only leaves when Send is pressed. Its own
 * component because a group needs exactly the same thing as a contact, and two
 * copies would be two places to fix anything twice.
 */
import type { PickedFile } from "../types";
import { canSend, describeProblem, describeSize } from "../types";

const props = defineProps<{
  files: PickedFile[];
  /** Who they are going to, for the button that sends them. */
  recipient: string;
}>();

const emit = defineEmits<{
  send: [];
  clear: [];
  remove: [path: string];
}>();

const sendable = () => props.files.filter(canSend).length;
</script>

<template>
  <div v-if="files.length" class="tray">
    <div class="tray-header">
      <span class="tray-title">
        {{ files.length }} {{ files.length === 1 ? "file" : "files" }} ready to send
      </span>

      <button class="tray-clear" @click="emit('clear')">Clear</button>
      <button
        class="tray-send"
        :disabled="sendable() === 0"
        :title="sendable() === 0 ? 'None of these can be sent' : `Send to ${recipient}`"
        @click="emit('send')"
      >
        Send {{ sendable() }}
      </button>
    </div>

    <ul class="tray-list">
      <li
        v-for="file in files"
        :key="file.path"
        class="tray-row"
        :class="{ rejected: !canSend(file) }"
      >
        <span class="file-name" :title="file.path">{{ file.name }}</span>
        <span class="tray-meta">
          {{ describeProblem(file) ?? `${describeSize(file.size)} · not sent yet` }}
        </span>

        <button class="tray-remove" title="Remove from this batch" @click="emit('remove', file.path)">
          <svg
            viewBox="0 0 24 24"
            width="13"
            height="13"
            stroke="currentColor"
            stroke-width="2.2"
            fill="none"
            stroke-linecap="round"
          >
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.tray {
  margin-bottom: 8px;
  border: 1px dashed var(--border-strong);
  border-radius: var(--radius);
  background-color: var(--bg);
}

.tray-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
}

.tray-title {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-muted);
}

.tray-clear {
  flex: none;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  color: var(--text-faint);
}

.tray-clear:hover {
  background-color: var(--bg-hover);
  color: var(--text);
}

.tray-send {
  flex: none;
  padding: 5px 12px;
  border-radius: var(--radius-pill);
  background-color: var(--accent);
  font-size: 12px;
  font-weight: 500;
  color: var(--accent-contrast);
}

.tray-send:hover:not(:disabled) {
  background-color: var(--accent-hover);
}

.tray-send:disabled {
  background-color: var(--bg-hover);
  color: var(--text-faint);
}

.tray-list {
  margin: 0;
  padding: 0;
  list-style: none;
}

.tray-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 12px;
}

.tray-row + .tray-row {
  border-top: 1px solid var(--border);
}

.file-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tray-meta {
  flex: none;
  font-size: 11px;
  color: var(--text-faint);
}

/* Something that will not go out, said at the point of picking rather than as a
   failure after the fact. */
.tray-row.rejected .file-name {
  color: var(--text-faint);
  text-decoration: line-through;
}

.tray-row.rejected .tray-meta {
  color: var(--danger);
}

.tray-remove {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--text-faint);
}

.tray-remove:hover {
  background-color: var(--bg-hover);
  color: var(--text);
}
</style>
