<script setup lang="ts">
/**
 * One chat message.
 *
 * Outgoing messages carry a delivery status in the corner: a clock while in
 * flight, one check once the other node accepted it, two once it was read.
 */
import type { ChatMessage } from "../types";

defineProps<{
  message: ChatMessage;
  /** True when we wrote this message, false when it came from the other peer. */
  outgoing: boolean;
  /**
   * Who wrote it, shown above the text. Only passed in groups, where a message
   * could be from any member; direct chats have an obvious sender.
   */
  senderLabel?: string;
}>();

const emit = defineEmits<{
  /** Asked for by clicking a message that didn't go out. */
  retry: [];
}>();

const STATUS_LABELS: Record<ChatMessage["status"], string> = {
  sending: "Sending",
  delivered: "Delivered",
  read: "Read",
  failed: "Not sent",
};
</script>

<template>
  <div class="line" :class="outgoing ? 'from-us' : 'from-them'">
    <div class="bubble" :class="outgoing ? 'outgoing' : 'incoming'">
    <span class="body">
      <span v-if="senderLabel" class="sender">{{ senderLabel }}</span>
      <span class="text">{{ message.text }}</span>
    </span>

    <span
      v-if="outgoing"
      class="status"
      :class="{ read: message.status === 'read' }"
      :title="STATUS_LABELS[message.status]"
    >

      <!-- Clock: queued or in flight. -->
      <svg
        v-if="message.status === 'sending'"
        viewBox="0 0 24 24"
        width="12"
        height="12"
        stroke="currentColor"
        stroke-width="2.5"
        fill="none"
      >
        <circle cx="12" cy="12" r="10" />
        <polyline points="12 6 12 12 16 14" />
      </svg>

      <!-- One check: the other node accepted it. -->
      <svg
        v-else-if="message.status === 'delivered'"
        viewBox="0 0 24 24"
        width="12"
        height="12"
        stroke="currentColor"
        stroke-width="3"
        fill="none"
      >
        <polyline points="20 6 9 17 4 12" />
      </svg>

      <!-- Two checks: the other node read it. -->
      <svg
        v-else
        viewBox="0 0 24 24"
        width="14"
        height="14"
        stroke="currentColor"
        stroke-width="3"
        fill="none"
      >
        <polyline points="18 6 7 17 2 12" />
        <polyline points="22 6 12 16 11 15" />
      </svg>
      </span>
    </div>

    <!-- Said in words underneath rather than as an icon inside. A message that
         did not arrive is the one status somebody has to act on, and an icon in
         a corner is not where anybody looks for that. -->
    <p v-if="outgoing && message.status === 'failed'" class="undelivered">
      Not delivered.
      <button class="retry" @click="emit('retry')">Try again</button>
    </p>
  </div>
</template>

<style scoped>
/* Wraps a bubble and anything said about it, so both sit on the same side of
   the conversation. */
.line {
  display: flex;
  flex-direction: column;
  max-width: min(70%, 560px);
}

.line.from-us {
  align-self: flex-end;
  align-items: flex-end;
}

.line.from-them {
  align-self: flex-start;
  align-items: flex-start;
}

.undelivered {
  margin: 3px 2px 0;
  font-size: 11px;
  color: var(--danger);
}

.retry {
  padding: 0;
  font-size: 11px;
  font-weight: 600;
  color: var(--danger);
  text-decoration: underline;
}

.retry:hover {
  color: var(--text);
}

.bubble {
  display: flex;
  align-items: flex-end;
  gap: 8px;
  max-width: 100%;
  width: fit-content;
  padding: 7px 12px;
  border-radius: var(--radius-bubble);
}

.outgoing {
  border-bottom-right-radius: 4px;
  background-color: var(--bubble-out-bg);
  color: var(--bubble-out-text);
}

.incoming {
  border-bottom-left-radius: 4px;
  background-color: var(--bubble-in-bg);
  color: var(--bubble-in-text);
}

.body {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.sender {
  font-size: 11px;
  font-weight: 600;
  opacity: 0.75;
}

.text {
  /* Long words and pasted peer IDs wrap instead of stretching the bubble. */
  overflow-wrap: anywhere;
  white-space: pre-wrap;
}

.status {
  display: flex;
  align-items: center;
  flex: none;
  /* Sits on the baseline of the last line of text. */
  padding-bottom: 2px;
  opacity: 0.7;
}

.status.read {
  opacity: 1;
}
</style>
