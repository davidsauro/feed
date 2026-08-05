<script setup lang="ts">
/**
 * The open conversation: header, scrolling history, and the composer.
 *
 * Serves both direct chats and groups. What differs between them is passed in
 * — presence only applies to a person, and sender labels only to a group — so
 * the history, composer, and scrolling behavior are written once.
 *
 * Sending is reported upward. All this component owns is the draft text and
 * keeping the history scrolled to the newest message.
 */
import { nextTick, ref, watch } from "vue";
import MessageBubble from "./MessageBubble.vue";
import type { ChatMessage } from "../types";

const props = defineProps<{
  /** Contact nickname, or group name. */
  title: string;
  /** Shortened peer ID, or the member count. */
  subtitle: string;
  messages: ChatMessage[];
  /** Our own peer ID, used to tell our messages from theirs. */
  myPeerId: string;
  /**
   * Whether the other side is reachable, or null in a group, where presence of
   * one member says nothing useful about the conversation.
   */
  online?: boolean | null;
  /**
   * Peer ID to display name, used to attribute incoming messages in a group.
   * Absent in direct chats, where every incoming message is from the same
   * person named in the header.
   */
  senderLabels?: Record<string, string>;
}>();

const emit = defineEmits<{
  send: [text: string];
}>();

const draft = ref("");
const history = ref<HTMLElement | null>(null);

function send() {
  const text = draft.value.trim();
  if (!text) {
    return;
  }

  emit("send", text);
  draft.value = "";
}

/**
 * Pins the history to the bottom whenever a message arrives or the
 * conversation changes, which is what makes the newest message the one you see.
 */
watch(
  () => [props.title, props.messages.length],
  async () => {
    await nextTick();
    if (history.value) {
      history.value.scrollTop = history.value.scrollHeight;
    }
  },
  { immediate: true },
);
</script>

<template>
  <section class="chat">
    <header class="header">
      <div class="who">
        <h2 class="nickname">{{ title }}</h2>
        <code class="peer-id">{{ subtitle }}</code>
      </div>

      <span v-if="online !== null && online !== undefined" class="presence" :class="{ online }">
        <span class="status-dot" />
        {{ online ? "Online" : "Offline" }}
      </span>
    </header>

    <div ref="history" class="history">
      <p v-if="messages.length === 0" class="empty">
        No messages yet. Say hello.
      </p>

      <MessageBubble
        v-for="message in messages"
        :key="message.id"
        :message="message"
        :outgoing="message.sender === myPeerId"
        :sender-label="senderLabels?.[message.sender]"
      />
    </div>

    <footer class="composer">
      <input
        v-model="draft"
        type="text"
        placeholder="Write a message…"
        @keyup.enter="send"
      />
      <button
        class="send"
        title="Send"
        :disabled="!draft.trim()"
        @click="send"
      >
        <svg
          viewBox="0 0 24 24"
          width="16"
          height="16"
          stroke="currentColor"
          stroke-width="2"
          fill="none"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <line x1="4" y1="12" x2="19" y2="12" />
          <polyline points="13 6 19 12 13 18" />
        </svg>
      </button>
    </footer>
  </section>
</template>

<style scoped>
.chat {
  display: flex;
  flex-direction: column;
  /* min-height on a flex child is what lets the history scroll instead of
     stretching the whole window. */
  min-height: 0;
  height: 100%;
}

.header {
  display: flex;
  align-items: center;
  gap: 12px;
  flex: none;
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  background-color: var(--bg);
}

.who {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.nickname {
  margin: 0;
  overflow: hidden;
  font-size: 15px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.peer-id {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-faint);
}

.presence {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-left: auto;
  flex: none;
  font-size: 12px;
  color: var(--text-muted);
}

.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background-color: var(--offline);
}

.presence.online .status-dot {
  background-color: var(--online);
}

.history {
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex: 1;
  min-height: 0;
  padding: 16px;
  overflow-y: auto;
  background-color: var(--bg-sunken);
}

.empty {
  margin: auto;
  font-size: 13px;
  color: var(--text-faint);
}

.composer {
  display: flex;
  gap: 8px;
  flex: none;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
  background-color: var(--bg);
}

.composer input {
  flex: 1;
  padding: 9px 12px;
  border-radius: var(--radius-pill);
}

.send {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 36px;
  height: 36px;
  border-radius: 50%;
  background-color: var(--accent);
  color: var(--accent-contrast);
}

.send:hover:not(:disabled) {
  background-color: var(--accent-hover);
}
</style>
