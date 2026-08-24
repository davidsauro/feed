<script setup lang="ts">
/**
 * One file in a list, with whatever action makes sense for the state it is in.
 *
 * Its own component because it appears twice: under a contact in the flat list,
 * and under one member of a group once their transfers are expanded. Those are
 * different enough to lay out separately and identical in what a single row has
 * to say.
 */
import type { FileTransfer } from "../types";
import { describeSize, describeTransferError, describeWhen } from "../types";

const props = defineProps<{
  file: FileTransfer;
  /** Ids that had not been looked at when the view was opened. */
  newlyArrived: Set<string>;
}>();

const emit = defineEmits<{
  open: [file: FileTransfer];
  reveal: [file: FileTransfer];
  resume: [file: FileTransfer];
}>();

/** What this row says about where the file got to. */
function describeStatus(file: FileTransfer): string {
  const incoming = file.direction === "incoming";

  switch (file.status) {
    case "complete":
      return incoming ? "received" : "sent";
    case "transferring":
      return incoming ? "receiving" : "sending";
    case "pending":
      // The backend says what it is waiting on, which for a peer who is not
      // answering is the difference between a report and a frozen row.
      return file.error ?? "starting";
    case "offered":
      return file.error ?? "waiting for them";
    case "failed":
      return file.error ? describeTransferError(file.error) : "failed";
    default:
      return file.status;
  }
}

const progress = () =>
  props.file.size === 0
    ? 100
    : Math.min(100, Math.round((props.file.transferred / props.file.size) * 100));

const inFlight = () =>
  props.file.status === "transferring" || props.file.status === "pending";

const canResume = () => props.file.status === "failed";
</script>

<template>
  <li class="row" :class="file.status">
    <span class="direction" :class="file.direction" aria-hidden="true">
      <svg
        viewBox="0 0 24 24"
        width="13"
        height="13"
        stroke="currentColor"
        stroke-width="2.5"
        fill="none"
        stroke-linecap="round"
      >
        <line x1="12" y1="5" x2="12" y2="19" />
        <polyline
          :points="file.direction === 'incoming' ? '6 13 12 19 18 13' : '6 11 12 5 18 11'"
        />
      </svg>
    </span>

    <span class="details">
      <span class="file-line">
        <span class="file-name" :title="file.name">{{ file.name }}</span>
        <!-- Turned up while you were somewhere else. -->
        <span v-if="newlyArrived.has(file.id)" class="new">New</span>
      </span>

      <!-- The full text on hover, since the detail is what somebody wants at
           exactly the moment they are working out why. -->
      <span class="meta" :title="file.error ?? undefined">
        {{ describeSize(file.size) }} · {{ describeStatus(file) }} ·
        {{ describeWhen(file.sent_at) }}
      </span>

      <span v-if="inFlight()" class="progress">
        <span class="bar" :style="{ width: `${progress()}%` }" />
      </span>
    </span>

    <span v-if="canResume()" class="actions">
      <button class="resume" title="Ask for the rest" @click="emit('resume', file)">
        Resume
      </button>
    </span>

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
          <path
            d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"
          />
        </svg>
      </button>
    </span>
  </li>
</template>

<style scoped>
.row {
  display: flex;
  align-items: center;
  gap: 11px;
  padding: 9px 12px;
}

.row + .row {
  border-top: 1px solid var(--border);
}

.direction {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 22px;
  height: 22px;
  border-radius: 50%;
}

.direction.incoming {
  background-color: var(--bg-active);
  color: var(--accent);
}

.direction.outgoing {
  background-color: var(--bg-hover);
  color: var(--text-muted);
}

.details {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-width: 0;
}

.file-line {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}

.file-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.new {
  flex: none;
  padding: 1px 6px;
  border-radius: var(--radius-pill);
  background-color: var(--accent);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.02em;
  color: var(--accent-contrast);
}

.meta {
  font-size: 11px;
  color: var(--text-faint);
  /* A failure can carry an address, which has no spaces to break at and would
     otherwise run straight through the buttons beside it. */
  overflow-wrap: anywhere;
}

.row.failed .meta {
  color: var(--danger);
}

.progress {
  display: block;
  height: 3px;
  margin-top: 3px;
  border-radius: var(--radius-pill);
  background-color: var(--border);
  overflow: hidden;
}

.bar {
  display: block;
  height: 100%;
  border-radius: var(--radius-pill);
  background-color: var(--accent);
  transition: width 0.2s ease;
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
  width: 26px;
  height: 26px;
  border-radius: var(--radius-sm);
  color: var(--text-faint);
}

.action:hover {
  background-color: var(--bg-hover);
  color: var(--text);
}

.resume {
  padding: 4px 10px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  font-size: 12px;
  font-weight: 500;
  color: var(--text);
}

.resume:hover {
  background-color: var(--bg-hover);
}
</style>
