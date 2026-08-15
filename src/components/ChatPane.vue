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
import { computed, nextTick, ref, watch } from "vue";
import FileBubble from "./FileBubble.vue";
import MessageBubble from "./MessageBubble.vue";
import type { ChatMessage, FileTransfer } from "../types";

const props = defineProps<{
  /** Contact nickname, or group name. */
  title: string;
  /** Shortened peer ID, or the member count. */
  subtitle: string;
  messages: ChatMessage[];
  /** Files sent or received in this conversation. */
  files: FileTransfer[];
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
  /** Shows the button for adding people. Groups only. */
  canAddMembers?: boolean;
}>();

const emit = defineEmits<{
  send: [text: string];
  addMembers: [];
  /** A message that didn't go out should be tried again. */
  retry: [id: string];
  attach: [];
  openFile: [file: FileTransfer];
  revealFile: [file: FileTransfer];
  resumeFile: [file: FileTransfer];
}>();

/**
 * Messages and files in one list, in the order they were sent.
 *
 * Sending somebody a file is a thing you said to them, so it belongs in the
 * conversation rather than only in a separate list. Merging here rather than
 * storing a file as a message too keeps one record of a transfer rather than two
 * that could disagree.
 */
const timeline = computed(() => {
  const entries = [
    ...props.messages.map((message) => ({
      key: `m:${message.id}`,
      sentAt: message.sent_at,
      message,
      file: null as FileTransfer | null,
    })),
    ...props.files.map((file) => ({
      key: `f:${file.id}`,
      sentAt: file.sent_at,
      message: null as ChatMessage | null,
      file,
    })),
  ];

  return entries.sort((a, b) => a.sentAt - b.sentAt);
});

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
  () => [props.title, props.messages.length, props.files.length],
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

      <button
        v-if="canAddMembers"
        class="add-members"
        title="Add people to this group"
        @click="emit('addMembers')"
      >
        <svg
          viewBox="0 0 24 24"
          width="15"
          height="15"
          stroke="currentColor"
          stroke-width="2"
          fill="none"
          stroke-linecap="round"
        >
          <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
          <circle cx="9" cy="7" r="4" />
          <line x1="19" y1="8" x2="19" y2="14" />
          <line x1="22" y1="11" x2="16" y2="11" />
        </svg>
      </button>
    </header>

    <div ref="history" class="history">
      <p v-if="timeline.length === 0" class="empty">
        No messages yet. Say hello.
      </p>

      <template v-for="entry in timeline" :key="entry.key">
        <MessageBubble
          v-if="entry.message"
          :message="entry.message"
          :outgoing="entry.message.sender === myPeerId"
          :sender-label="senderLabels?.[entry.message.sender]"
          @retry="emit('retry', entry.message!.id)"
        />

        <FileBubble
          v-else-if="entry.file"
          :file="entry.file"
          :outgoing="entry.file.direction === 'outgoing'"
          @open="emit('openFile', $event)"
          @reveal="emit('revealFile', $event)"
          @resume="emit('resumeFile', $event)"
        />
      </template>
    </div>

    <footer class="composer">
      <button class="attach" title="Send files" @click="emit('attach')">
        <svg
          viewBox="0 0 24 24"
          width="17"
          height="17"
          stroke="currentColor"
          stroke-width="2"
          fill="none"
          stroke-linecap="round"
        >
          <path d="M21.4 11.05 12.25 20.2a6 6 0 0 1-8.49-8.49l9.2-9.19a4 4 0 0 1 5.65 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
        </svg>
      </button>

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

.add-members {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 28px;
  height: 28px;
  margin-left: auto;
  border-radius: var(--radius-sm);
  color: var(--text-faint);
}

.add-members:hover {
  background-color: var(--bg-hover);
  color: var(--text);
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

.attach {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 36px;
  height: 36px;
  border-radius: 50%;
  color: var(--text-faint);
}

.attach:hover {
  background-color: var(--bg-hover);
  color: var(--text);
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
