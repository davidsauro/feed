<script setup lang="ts">
/**
 * A file in a conversation.
 *
 * Sits where a message bubble would, because from the reader's point of view
 * sending somebody a file is a thing you said to them.
 */
import type { FileTransfer } from "../types";
import { describeSize } from "../types";

const props = defineProps<{
  file: FileTransfer;
  outgoing: boolean;
}>();

const emit = defineEmits<{
  open: [file: FileTransfer];
  reveal: [file: FileTransfer];
  resume: [file: FileTransfer];
}>();

/** Only the receiving side can ask for the rest, since only it knows how much
 * it already has. */
const canResume = () =>
  props.file.direction === "incoming" && props.file.status === "failed";

/** How far along, as a percentage, for the bar under an active transfer. */
const progress = () => {
  if (props.file.size === 0) {
    return 100;
  }

  return Math.min(100, Math.round((props.file.transferred / props.file.size) * 100));
};

const inFlight = () =>
  props.file.status === "transferring" || props.file.status === "pending";

/** What the second line says, which is the only place status is spelled out. */
const detail = () => {
  const size = describeSize(props.file.size);

  switch (props.file.status) {
    case "offered":
      return `${size}, waiting for them to fetch it`;
    case "pending":
      return `${size}, starting`;
    case "transferring":
      return `${describeSize(props.file.transferred)} of ${size}`;
    case "failed":
      return props.file.error ?? "did not arrive";
    default:
      return size;
  }
};
</script>

<template>
  <div class="bubble" :class="[outgoing ? 'outgoing' : 'incoming', file.status]">
    <div class="row">
      <svg
        class="icon"
        viewBox="0 0 24 24"
        width="18"
        height="18"
        stroke="currentColor"
        stroke-width="1.8"
        fill="none"
      >
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
        <polyline points="14 2 14 8 20 8" />
      </svg>

      <span class="text">
        <span class="name" :title="file.name">{{ file.name }}</span>
        <span class="detail">{{ detail() }}</span>
      </span>

      <span v-if="canResume()" class="actions">
        <button class="resume" title="Ask for the rest" @click="emit('resume', file)">
          Resume
        </button>
      </span>

      <!-- Only once it is actually on this machine. Offering a button that
           cannot work is worse than offering none. -->
      <span v-else-if="file.status === 'complete'" class="actions">
        <button class="action" title="Open" @click="emit('open', file)">
          <svg
            viewBox="0 0 24 24"
            width="14"
            height="14"
            stroke="currentColor"
            stroke-width="2"
            fill="none"
            stroke-linecap="round"
          >
            <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
            <polyline points="15 3 21 3 21 9" />
            <line x1="10" y1="14" x2="21" y2="3" />
          </svg>
        </button>

        <button class="action" title="Show in folder" @click="emit('reveal', file)">
          <svg
            viewBox="0 0 24 24"
            width="14"
            height="14"
            stroke="currentColor"
            stroke-width="2"
            fill="none"
          >
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
        </button>
      </span>
    </div>

    <div v-if="inFlight()" class="progress">
      <span class="bar" :style="{ width: `${progress()}%` }" />
    </div>
  </div>
</template>

<style scoped>
.bubble {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-width: min(78%, 420px);
  width: fit-content;
  padding: 9px 12px;
  border-radius: var(--radius-bubble);
}

.outgoing {
  align-self: flex-end;
  border-bottom-right-radius: 4px;
  background-color: var(--bubble-out-bg);
  color: var(--bubble-out-text);
}

.incoming {
  align-self: flex-start;
  border-bottom-left-radius: 4px;
  background-color: var(--bubble-in-bg);
  color: var(--bubble-in-text);
}

/* A transfer that did not make it should not look like one that did. */
.bubble.failed {
  border: 1px solid var(--danger);
}

.row {
  display: flex;
  align-items: center;
  gap: 10px;
}

.icon {
  flex: none;
  opacity: 0.8;
}

.text {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.name {
  overflow: hidden;
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.detail {
  font-size: 11px;
  opacity: 0.75;
}

.actions {
  display: flex;
  gap: 2px;
  flex: none;
}

.action {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  color: currentColor;
  opacity: 0.75;
}

.action:hover {
  background-color: rgba(127, 127, 127, 0.25);
  opacity: 1;
}

.resume {
  padding: 3px 9px;
  border: 1px solid currentColor;
  border-radius: var(--radius-sm);
  font-size: 11px;
  font-weight: 500;
  color: currentColor;
  opacity: 0.9;
}

.resume:hover {
  background-color: rgba(127, 127, 127, 0.25);
  opacity: 1;
}

.progress {
  height: 3px;
  border-radius: var(--radius-pill);
  background-color: rgba(127, 127, 127, 0.3);
  overflow: hidden;
}

.bar {
  display: block;
  height: 100%;
  border-radius: var(--radius-pill);
  background-color: currentColor;
  opacity: 0.8;
  transition: width 0.2s ease;
}
</style>
