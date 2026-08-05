<script setup lang="ts">
/**
 * Application shell.
 *
 * This component owns all state and every call into the Rust backend. The
 * components under it are presentational: they take props and report what the
 * user did, and this file decides what that means.
 */
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import ChatPane from "./components/ChatPane.vue";
import ConfirmDialog from "./components/ConfirmDialog.vue";
import ContactList from "./components/ContactList.vue";
import IdentityBar from "./components/IdentityBar.vue";
import PeerList from "./components/PeerList.vue";
import ThemeToggle from "./components/ThemeToggle.vue";
import type { ChatMessage, Contact } from "./types";

// --- State ----------------------------------------------------------------

const myPeerId = ref("");
const nodeInstance = ref("…");

/** Peer IDs mDNS can currently see, contacts and strangers alike. */
const activePeers = ref<Set<string>>(new Set());
const savedContacts = ref<Contact[]>([]);

/** Conversations, keyed by the other peer's ID. */
const messages = ref<Record<string, ChatMessage[]>>({});
/** Which contacts have a message we haven't shown yet. */
const unreadStatus = ref<Record<string, boolean>>({});

const selectedPeerId = ref<string | null>(null);

/** A short-lived message shown over the UI. Replaces blocking alert() dialogs. */
const notice = ref<{ text: string; kind: "error" | "info" } | null>(null);
let noticeTimer = 0;

/**
 * The contact the user has asked to remove, held here while the confirmation
 * dialog is open. Non-null is what makes the dialog visible.
 */
const pendingRemoval = ref<{ contact: Contact; messageCount: number } | null>(null);

// --- Derived state --------------------------------------------------------

/**
 * Looked up rather than stored, so a rename shows up in the chat header without
 * having to update two places.
 */
const selectedContact = computed<Contact | null>(() => {
  const found = savedContacts.value.find(
    (contact) => contact.peer_id === selectedPeerId.value,
  );

  return found ?? null;
});

/** Discovered peers we haven't saved as contacts yet. */
const unregisteredPeers = computed(() =>
  Array.from(activePeers.value).filter(
    (peer) => !savedContacts.value.some((contact) => contact.peer_id === peer),
  ),
);

const currentMessages = computed(() => {
  if (!selectedPeerId.value) {
    return [];
  }

  return messages.value[selectedPeerId.value] ?? [];
});

const selectedIsOnline = computed(
  () => selectedPeerId.value !== null && activePeers.value.has(selectedPeerId.value),
);

// --- Notices --------------------------------------------------------------

function notify(text: string, kind: "error" | "info" = "error") {
  notice.value = { text, kind };

  window.clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => {
    notice.value = null;
  }, 6000);
}

// --- Loading --------------------------------------------------------------

async function loadIdentity() {
  try {
    myPeerId.value = await invoke<string>("get_identity");
  } catch (error) {
    notify(`Could not load this node's identity: ${error}`);
  }
}

/**
 * Loads contacts, then checks each one's history for messages that arrived
 * while the app was closed so their unread dot survives a restart.
 */
async function loadContacts() {
  try {
    savedContacts.value = await invoke<Contact[]>("get_contacts");
  } catch (error) {
    notify(`Could not load contacts: ${error}`);
    return;
  }

  for (const contact of savedContacts.value) {
    try {
      const history = await invoke<ChatMessage[]>("get_chat_history", {
        peerId: contact.peer_id,
      });

      unreadStatus.value[contact.peer_id] = history.some(
        (message) => message.sender === contact.peer_id && message.status !== "read",
      );
    } catch (error) {
      console.error(`Could not check unread messages for ${contact.peer_id}`, error);
    }
  }
}

// --- Contacts -------------------------------------------------------------

async function addContact(peerId: string, nickname: string) {
  try {
    await invoke("save_contact", { peerId, nickname });
    await loadContacts();
    notify(`Added ${nickname}.`, "info");
  } catch (error) {
    notify(`Could not add contact: ${error}`);
  }
}

async function renameContact(peerId: string, nickname: string) {
  try {
    await invoke("save_contact", { peerId, nickname });
    await loadContacts();
  } catch (error) {
    notify(`Could not rename contact: ${error}`);
  }
}

/**
 * Step one of removing a contact: look up what would be lost and open the
 * confirmation. Nothing is deleted here.
 */
async function requestRemoval(contact: Contact) {
  let messageCount = 0;

  try {
    messageCount = await invoke<number>("count_chat_messages", {
      peerId: contact.peer_id,
    });
  } catch (error) {
    // Worth continuing without the count: the user can still make the decision,
    // and the warning covers the history either way.
    console.error("Could not count stored messages", error);
    messageCount = -1;
  }

  pendingRemoval.value = { contact, messageCount };
}

/** Step two: the user confirmed, so remove the contact and their history. */
async function confirmRemoval() {
  const pending = pendingRemoval.value;
  if (!pending) {
    return;
  }

  const { contact } = pending;
  pendingRemoval.value = null;

  try {
    const deleted = await invoke<number>("delete_contact", {
      peerId: contact.peer_id,
    });

    // Drop the local copies too, or the conversation would linger on screen
    // until the next restart.
    delete messages.value[contact.peer_id];
    delete unreadStatus.value[contact.peer_id];

    if (selectedPeerId.value === contact.peer_id) {
      selectedPeerId.value = null;
    }

    await loadContacts();

    notify(
      `Removed ${contact.nickname} and deleted ${describeMessageCount(deleted)}.`,
      "info",
    );
  } catch (error) {
    notify(`Could not remove ${contact.nickname}: ${error}`);
  }
}

/** "1 message" / "4 messages", so the dialog and toast read as sentences. */
function describeMessageCount(count: number): string {
  return count === 1 ? "1 message" : `${count} messages`;
}

/**
 * The consequence spelled out for the confirmation dialog.
 *
 * A count of -1 means the lookup failed; the warning still has to be clear that
 * history goes away.
 */
const removalWarning = computed(() => {
  const pending = pendingRemoval.value;
  if (!pending) {
    return "";
  }

  if (pending.messageCount < 0) {
    return "Your entire chat history with this contact will be permanently deleted. This cannot be undone.";
  }

  if (pending.messageCount === 0) {
    return "There is no chat history with this contact yet, so nothing else will be lost.";
  }

  return `${describeMessageCount(pending.messageCount)} will be permanently deleted along with them. This cannot be undone.`;
});

// --- Conversations --------------------------------------------------------

/**
 * Tells the other node which of its messages we've read. Sent as a hidden JSON
 * payload over the same channel as chat messages.
 */
async function sendReadReceipt(peerId: string, messageIds: string[]) {
  const payload = JSON.stringify({ type: "read", messageIds });

  try {
    await invoke("send_message", { peerId, message: payload });
  } catch (error) {
    // A receipt that doesn't arrive is not worth interrupting the user over;
    // they'll get one next time the conversation is opened.
    console.error("Could not send read receipt", error);
  }
}

/**
 * Opens a conversation: loads its history, clears the unread dot, and tells the
 * other side we've read what they sent.
 */
async function selectContact(contact: Contact) {
  selectedPeerId.value = contact.peer_id;

  try {
    messages.value[contact.peer_id] = await invoke<ChatMessage[]>("get_chat_history", {
      peerId: contact.peer_id,
    });
  } catch (error) {
    notify(`Could not load this conversation: ${error}`);
    return;
  }

  unreadStatus.value[contact.peer_id] = false;

  const unread = messages.value[contact.peer_id].filter(
    (message) => message.sender === contact.peer_id && message.status !== "read",
  );

  if (unread.length === 0) {
    return;
  }

  sendReadReceipt(
    contact.peer_id,
    unread.map((message) => message.id),
  );

  for (const message of unread) {
    message.status = "read";
    markMessageRead(message.id);
  }
}

/** Records a status change in SQLite. Best effort: the UI has already moved on. */
function markMessageRead(id: string) {
  invoke("update_message_status", { id, status: "read" }).catch((error) => {
    console.error(`Could not mark ${id} as read`, error);
  });
}

/**
 * Sends a message, showing it in the conversation right away.
 *
 * The message is saved as "sending" before it goes out so it survives a crash
 * mid-send, then promoted to "delivered" once the other node accepts it.
 */
async function sendMessage(text: string) {
  const contact = selectedContact.value;
  if (!contact) {
    return;
  }

  const peerId = contact.peer_id;
  const id = crypto.randomUUID();

  const message: ChatMessage = {
    id,
    sender: myPeerId.value,
    text,
    status: "sending",
  };

  if (!messages.value[peerId]) {
    messages.value[peerId] = [];
  }
  messages.value[peerId].push(message);

  try {
    await invoke("save_chat_message", {
      id,
      peerId,
      sender: myPeerId.value,
      text,
      status: "sending",
    });

    await invoke("send_message", {
      peerId,
      message: JSON.stringify({ type: "chat", id, text }),
    });

    message.status = "delivered";
    await invoke("update_message_status", { id, status: "delivered" });
  } catch (error) {
    notify(`Could not send to ${contact.nickname}: ${error}`);
  }
}

// --- Inbound network events ----------------------------------------------

/** Handles a chat message from a contact. */
async function receiveChatMessage(sender: string, id: string, text: string) {
  const message: ChatMessage = { id, sender, text, status: "delivered" };

  if (!messages.value[sender]) {
    messages.value[sender] = [];
  }
  messages.value[sender].push(message);

  await invoke("save_chat_message", {
    id,
    peerId: sender,
    sender,
    text,
    status: "delivered",
  });

  // If their conversation is already on screen, it's read the moment it lands.
  if (selectedPeerId.value === sender) {
    sendReadReceipt(sender, [id]);
    message.status = "read";
    markMessageRead(id);
  } else {
    unreadStatus.value[sender] = true;
  }
}

/** Handles the other side telling us they read our messages. */
function receiveReadReceipt(sender: string, messageIds: string[]) {
  const conversation = messages.value[sender];
  if (!conversation) {
    return;
  }

  for (const message of conversation) {
    if (messageIds.includes(message.id)) {
      message.status = "read";
      markMessageRead(message.id);
    }
  }
}

// --- Startup --------------------------------------------------------------

onMounted(async () => {
  nodeInstance.value = await invoke<string>("get_node_id");
  await loadIdentity();
  await loadContacts();

  // Catch up on peers discovered before this window started listening.
  try {
    const initialPeers = await invoke<string[]>("get_active_peers");
    initialPeers.forEach((peer) => activePeers.value.add(peer));
  } catch (error) {
    console.error("Could not load the current peer list", error);
  }

  await listen<string>("peer-discovered", (event) => {
    activePeers.value.add(event.payload);
  });

  await listen<string>("peer-lost", (event) => {
    activePeers.value.delete(event.payload);
  });

  await listen<{ sender: string; message: string }>("chat-received", async (event) => {
    const { sender, message } = event.payload;

    // The backend passes the payload through untouched, so anything malformed
    // gets dropped here rather than breaking the listener.
    let data: { type?: string; id?: string; text?: string; messageIds?: string[] };
    try {
      data = JSON.parse(message);
    } catch {
      console.error("Ignoring an unreadable payload from", sender);
      return;
    }

    if (data.type === "chat" && data.id && typeof data.text === "string") {
      await receiveChatMessage(sender, data.id, data.text);
    } else if (data.type === "read" && Array.isArray(data.messageIds)) {
      receiveReadReceipt(sender, data.messageIds);
    }
  });
});
</script>

<template>
  <div class="app">
    <aside class="sidebar">
      <IdentityBar :peer-id="myPeerId" />

      <ContactList
        class="fill"
        :contacts="savedContacts"
        :online-peers="activePeers"
        :unread="unreadStatus"
        :selected-peer-id="selectedPeerId"
        @select="selectContact"
        @rename="renameContact"
        @remove="requestRemoval"
      />

      <PeerList :peers="unregisteredPeers" @add="addContact" />

      <footer class="footer">
        <span class="node-badge">
          <span class="dot" />
          Node {{ nodeInstance }}
        </span>

        <ThemeToggle />
      </footer>
    </aside>

    <main class="content">
      <ChatPane
        v-if="selectedContact"
        :key="selectedContact.peer_id"
        :contact="selectedContact"
        :messages="currentMessages"
        :online="selectedIsOnline"
        :my-peer-id="myPeerId"
        @send="sendMessage"
      />

      <div v-else class="placeholder">
        <svg
          viewBox="0 0 24 24"
          width="40"
          height="40"
          stroke="currentColor"
          stroke-width="1.5"
          fill="none"
        >
          <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
        </svg>

        <p class="placeholder-title">No conversation open</p>
        <p class="placeholder-hint">
          Pick a contact, or add a discovered peer to start talking to them.
        </p>
      </div>
    </main>

    <ConfirmDialog
      v-if="pendingRemoval"
      title="Remove this contact?"
      :message="`${pendingRemoval.contact.nickname} will be removed from your contacts, and messages from them will be silently dropped until you add them again.`"
      :warning="removalWarning"
      confirm-label="Remove and delete history"
      @confirm="confirmRemoval"
      @cancel="pendingRemoval = null"
    />

    <!-- Errors and one-line confirmations. Replaces the old alert() calls. -->
    <Transition name="notice">
      <div v-if="notice" class="notice" :class="notice.kind">
        <span>{{ notice.text }}</span>
        <button class="dismiss" title="Dismiss" @click="notice = null">✕</button>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.app {
  display: flex;
  height: 100%;
  overflow: hidden;
  background-color: var(--bg);
}

/* Sidebar ---------------------------------------------------------------- */

.sidebar {
  display: flex;
  flex-direction: column;
  flex: none;
  width: 272px;
  min-height: 0;
  border-right: 1px solid var(--border);
  background-color: var(--bg-sidebar);
}

/* Applied to ContactList's root: it takes the leftover height and scrolls,
   which keeps the discovered list and node badge pinned to the bottom. */
.fill {
  flex: 1;
  min-height: 0;
}

.footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  flex: none;
  padding: 7px 10px 7px 14px;
  border-top: 1px solid var(--border);
}

.node-badge {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-faint);
}

.dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background-color: var(--online);
}

/* Main area ------------------------------------------------------------- */

.content {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  min-height: 0;
}

.placeholder {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 4px;
  flex: 1;
  padding: 24px;
  text-align: center;
  background-color: var(--bg-sunken);
  color: var(--text-faint);
}

.placeholder-title {
  margin: 8px 0 0;
  font-weight: 600;
  color: var(--text-muted);
}

.placeholder-hint {
  margin: 0;
  max-width: 34ch;
  font-size: 13px;
}

/* Notices --------------------------------------------------------------- */

.notice {
  display: flex;
  align-items: center;
  gap: 10px;
  position: fixed;
  top: 12px;
  left: 50%;
  transform: translateX(-50%);
  max-width: min(90vw, 520px);
  padding: 8px 8px 8px 14px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius);
  background-color: var(--bg);
  box-shadow: var(--shadow);
  font-size: 13px;
}

.notice.error {
  border-color: var(--danger);
  background-color: var(--danger-bg);
  color: var(--danger);
}

.dismiss {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  opacity: 0.7;
}

.dismiss:hover {
  background-color: var(--bg-hover);
  opacity: 1;
}

.notice-enter-active,
.notice-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}

.notice-enter-from,
.notice-leave-to {
  opacity: 0;
  transform: translate(-50%, -8px);
}
</style>
