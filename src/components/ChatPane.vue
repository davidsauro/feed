<script setup lang="ts">
/**
 * The conversation with the selected contact: header, scrolling history, and
 * the composer.
 *
 * Sending is reported upward. All this component owns is the draft text and
 * keeping the history scrolled to the newest message.
 */
import { nextTick, ref, watch } from "vue";
import MessageBubble from "./MessageBubble.vue";
import type { ChatMessage, Contact } from "../types";
import { shortPeerId } from "../types";

const props = defineProps<{
  contact: Contact;
  messages: ChatMessage[];
  /** True when mDNS can currently see this contact. */
  online: boolean;
  /** Our own peer ID, used to tell our messages from theirs. */
  myPeerId: string;
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
  () => [props.contact.peer_id, props.messages.length],
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
        <h2 class="nickname">{{ contact.nickname }}</h2>
        <code class="peer-id" :title="contact.peer_id">
          {{ shortPeerId(contact.peer_id) }}
        </code>
      </div>

      <span class="presence" :class="{ online }">
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
