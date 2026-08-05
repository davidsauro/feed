<script setup lang="ts">
/**
 * Application shell.
 *
 * This component owns all state and every call into the Rust backend. The
 * components under it are presentational: they take props and report what the
 * user did, and this file decides what that means.
 *
 * Two kinds of conversation live here. Direct messages go peer to peer over the
 * request-response protocol. Group messages go over gossipsub, where they can
 * reach members we have no direct connection to, because peers in the middle
 * relay them.
 */
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import ChatPane from "./components/ChatPane.vue";
import ConfirmDialog from "./components/ConfirmDialog.vue";
import ContactList from "./components/ContactList.vue";
import GroupList from "./components/GroupList.vue";
import IdentityBar from "./components/IdentityBar.vue";
import NewGroupDialog from "./components/NewGroupDialog.vue";
import PeerList from "./components/PeerList.vue";
import ThemeToggle from "./components/ThemeToggle.vue";
import type { ChatMessage, Contact, Group } from "./types";
import { shortPeerId } from "./types";

/** Which kind of thing a conversation is with. */
type ConversationKind = "contact" | "group";

// --- State ----------------------------------------------------------------

const myPeerId = ref("");
const nodeInstance = ref("…");

/** Peer IDs mDNS can currently see, contacts and strangers alike. */
const activePeers = ref<Set<string>>(new Set());
const savedContacts = ref<Contact[]>([]);
const groups = ref<Group[]>([]);

/**
 * Conversations and unread flags, keyed by `conversationKey`.
 *
 * Direct chats and groups share these maps, so the key carries the kind as well
 * as the id rather than relying on peer IDs and group IDs never colliding.
 */
const messages = ref<Record<string, ChatMessage[]>>({});
const unread = ref<Record<string, boolean>>({});

/** The open conversation, or null for the empty state. */
const selection = ref<{ kind: ConversationKind; id: string } | null>(null);

/** A short-lived message shown over the UI. Replaces blocking alert() dialogs. */
const notice = ref<{ text: string; kind: "error" | "info" } | null>(null);
let noticeTimer = 0;

/**
 * What the user has asked to delete, held here while the confirmation dialog is
 * open. Non-null is what makes the dialog visible.
 */
const pendingRemoval = ref<{
  kind: ConversationKind;
  id: string;
  name: string;
  messageCount: number;
} | null>(null);

const creatingGroup = ref(false);

/** Identifies a conversation across the `messages` and `unread` maps. */
function conversationKey(kind: ConversationKind, id: string): string {
  return `${kind}:${id}`;
}

// --- Derived state --------------------------------------------------------

/**
 * Looked up rather than stored, so a rename shows up in the chat header without
 * having to update two places.
 */
const selectedContact = computed<Contact | null>(() => {
  if (selection.value?.kind !== "contact") {
    return null;
  }

  return (
    savedContacts.value.find((contact) => contact.peer_id === selection.value?.id) ?? null
  );
});

const selectedGroup = computed<Group | null>(() => {
  if (selection.value?.kind !== "group") {
    return null;
  }

  return groups.value.find((group) => group.id === selection.value?.id) ?? null;
});

/** Discovered peers we haven't saved as contacts yet. */
const unregisteredPeers = computed(() =>
  Array.from(activePeers.value).filter(
    (peer) => !savedContacts.value.some((contact) => contact.peer_id === peer),
  ),
);

const currentMessages = computed(() => {
  if (!selection.value) {
    return [];
  }

  return messages.value[conversationKey(selection.value.kind, selection.value.id)] ?? [];
});

const selectedIsOnline = computed(
  () => selectedContact.value !== null && activePeers.value.has(selectedContact.value.peer_id),
);

/** Unread flags for one kind, keyed by bare id the way the lists expect them. */
function unreadByKind(kind: ConversationKind, ids: string[]): Record<string, boolean> {
  const flags: Record<string, boolean> = {};

  for (const id of ids) {
    flags[id] = unread.value[conversationKey(kind, id)] ?? false;
  }

  return flags;
}

const contactUnread = computed(() =>
  unreadByKind("contact", savedContacts.value.map((contact) => contact.peer_id)),
);

const groupUnread = computed(() =>
  unreadByKind("group", groups.value.map((group) => group.id)),
);

/** What to call a peer: their nickname if we have one, otherwise a short ID. */
function displayName(peerId: string): string {
  if (peerId === myPeerId.value) {
    return "You";
  }

  const contact = savedContacts.value.find((saved) => saved.peer_id === peerId);

  return contact?.nickname ?? shortPeerId(peerId);
}

/**
 * Names for everyone in the open group, so each message can be attributed.
 *
 * A member we haven't added as a contact still gets a label here, even though
 * their messages are dropped before they reach us — they're visible in the
 * member count, so they shouldn't show up as a bare ID if that ever changes.
 */
const groupSenderLabels = computed<Record<string, string>>(() => {
  const labels: Record<string, string> = {};

  for (const member of selectedGroup.value?.members ?? []) {
    labels[member] = displayName(member);
  }

  return labels;
});

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

      unread.value[conversationKey("contact", contact.peer_id)] = history.some(
        (message) => message.sender === contact.peer_id && message.status !== "read",
      );
    } catch (error) {
      console.error(`Could not check unread messages for ${contact.peer_id}`, error);
    }
  }
}

async function loadGroups() {
  try {
    groups.value = await invoke<Group[]>("get_groups");
  } catch (error) {
    notify(`Could not load groups: ${error}`);
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

// --- Groups ---------------------------------------------------------------

/**
 * Creates a group and invites its members.
 *
 * The invite goes over the direct channel rather than gossipsub, because
 * gossipsub only delivers to peers already subscribed to a topic: a node that
 * has never heard of the group isn't subscribed, so the first message would go
 * nowhere. Once invited, everything else flows over gossipsub.
 */
async function createGroup(name: string, memberPeerIds: string[]) {
  creatingGroup.value = false;

  const id = crypto.randomUUID();
  const members = [myPeerId.value, ...memberPeerIds];

  try {
    await invoke("save_group", { id, name, members });
    await invoke("subscribe_group", { groupId: id });
    await loadGroups();
  } catch (error) {
    notify(`Could not create the group: ${error}`);
    return;
  }

  const invite = JSON.stringify({
    type: "group-invite",
    groupId: id,
    groupName: name,
    members,
  });

  const uninvited: string[] = [];
  for (const peerId of memberPeerIds) {
    try {
      await invoke("send_message", { peerId, message: invite });
    } catch (error) {
      console.error(`Could not invite ${peerId}`, error);
      uninvited.push(peerId);
    }
  }

  selection.value = { kind: "group", id };

  if (uninvited.length > 0) {
    // Not fatal: they'll be added by the member list on the next message they
    // do receive, once they're reachable again.
    notify(
      `Created ${name}, but could not reach ${uninvited.map(displayName).join(", ")}.`,
    );
  } else {
    notify(`Created ${name}.`, "info");
  }
}

/** Handles an invite from a contact by joining the group they made. */
async function receiveInvite(
  sender: string,
  groupId: string,
  groupName: string,
  members: string[],
) {
  try {
    await invoke("save_group", { id: groupId, name: groupName, members });
    await invoke("subscribe_group", { groupId });
    await loadGroups();

    notify(`${displayName(sender)} added you to ${groupName}.`, "info");
  } catch (error) {
    console.error("Could not join a group we were invited to", error);
  }
}

/**
 * Publishes to the open group.
 *
 * Unlike a direct message this can fail loudly: gossipsub refuses to publish
 * when no other member is subscribed, which is worth showing rather than
 * pretending the message went out.
 */
async function sendGroupMessage(text: string) {
  const group = selectedGroup.value;
  if (!group) {
    return;
  }

  const id = crypto.randomUUID();
  const message: ChatMessage = {
    id,
    sender: myPeerId.value,
    text,
    status: "sending",
  };

  const key = conversationKey("group", group.id);
  if (!messages.value[key]) {
    messages.value[key] = [];
  }
  messages.value[key].push(message);

  try {
    await invoke("save_group_message", {
      id,
      groupId: group.id,
      sender: myPeerId.value,
      text,
      status: "sending",
    });

    // The name and members ride along so anyone whose copy is out of date
    // catches up without a separate sync.
    await invoke("send_group_message", {
      groupId: group.id,
      message: JSON.stringify({
        type: "group-chat",
        id,
        text,
        groupName: group.name,
        members: group.members,
      }),
    });

    message.status = "delivered";
    await invoke("update_message_status", { id, status: "delivered" });
  } catch (error) {
    message.status = "failed";
    invoke("update_message_status", { id, status: "failed" }).catch(() => {});
    notify(`Could not send to ${group.name}: ${error}`);
  }
}

/**
 * Whether an inbound message describes a group differently to how we have it.
 *
 * Members are compared as sets, since nothing guarantees the order two nodes
 * store them in.
 */
function groupDetailsChanged(groupId: string, name: string, members: string[]): boolean {
  const known = groups.value.find((group) => group.id === groupId);
  if (!known) {
    return true;
  }

  if (known.name !== name || known.members.length !== members.length) {
    return true;
  }

  const knownMembers = new Set(known.members);

  return members.some((member) => !knownMembers.has(member));
}

/** Handles a group message from a contact. */
async function receiveGroupMessage(
  groupId: string,
  sender: string,
  id: string,
  text: string,
  groupName?: string,
  members?: string[],
) {
  // Keep our copy of the group in step with the sender's, but only write when
  // something actually changed: this runs on every inbound message.
  if (groupName && Array.isArray(members) && groupDetailsChanged(groupId, groupName, members)) {
    try {
      await invoke("save_group", { id: groupId, name: groupName, members });
      await loadGroups();
    } catch (error) {
      console.error("Could not update group details", error);
    }
  }

  const key = conversationKey("group", groupId);
  if (!messages.value[key]) {
    messages.value[key] = [];
  }

  // Gossipsub can deliver the same message twice on a mesh with loops.
  if (messages.value[key].some((message) => message.id === id)) {
    return;
  }

  messages.value[key].push({ id, sender, text, status: "delivered" });

  await invoke("save_group_message", {
    id,
    groupId,
    sender,
    text,
    status: "delivered",
  });

  if (selection.value?.kind !== "group" || selection.value.id !== groupId) {
    unread.value[key] = true;
  }
}

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
 * Opens a direct conversation: loads its history, clears the unread dot, and
 * tells the other side we've read what they sent.
 */
async function selectContact(contact: Contact) {
  selection.value = { kind: "contact", id: contact.peer_id };
  const key = conversationKey("contact", contact.peer_id);

  try {
    messages.value[key] = await invoke<ChatMessage[]>("get_chat_history", {
      peerId: contact.peer_id,
    });
  } catch (error) {
    notify(`Could not load this conversation: ${error}`);
    return;
  }

  unread.value[key] = false;

  const unreadMessages = messages.value[key].filter(
    (message) => message.sender === contact.peer_id && message.status !== "read",
  );

  if (unreadMessages.length === 0) {
    return;
  }

  sendReadReceipt(
    contact.peer_id,
    unreadMessages.map((message) => message.id),
  );

  for (const message of unreadMessages) {
    message.status = "read";
    markMessageRead(message.id);
  }
}

/**
 * Opens a group conversation.
 *
 * No read receipts here: gossipsub tells us a message reached the mesh, not who
 * read it, and tracking that per member is a bigger feature than it looks.
 */
async function selectGroup(group: Group) {
  selection.value = { kind: "group", id: group.id };
  const key = conversationKey("group", group.id);

  try {
    messages.value[key] = await invoke<ChatMessage[]>("get_group_history", {
      groupId: group.id,
    });
  } catch (error) {
    notify(`Could not load this conversation: ${error}`);
    return;
  }

  unread.value[key] = false;
}

/** Records a status change in SQLite. Best effort: the UI has already moved on. */
function markMessageRead(id: string) {
  invoke("update_message_status", { id, status: "read" }).catch((error) => {
    console.error(`Could not mark ${id} as read`, error);
  });
}

/**
 * Sends a direct message, showing it in the conversation right away.
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

  const key = conversationKey("contact", peerId);
  if (!messages.value[key]) {
    messages.value[key] = [];
  }
  messages.value[key].push(message);

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
    message.status = "failed";
    invoke("update_message_status", { id, status: "failed" }).catch(() => {});
    notify(`Could not send to ${contact.nickname}: ${error}`);
  }
}

/** Handles a direct chat message from a contact. */
async function receiveChatMessage(sender: string, id: string, text: string) {
  const key = conversationKey("contact", sender);
  const message: ChatMessage = { id, sender, text, status: "delivered" };

  if (!messages.value[key]) {
    messages.value[key] = [];
  }
  messages.value[key].push(message);

  await invoke("save_chat_message", {
    id,
    peerId: sender,
    sender,
    text,
    status: "delivered",
  });

  // If their conversation is already on screen, it's read the moment it lands.
  if (selection.value?.kind === "contact" && selection.value.id === sender) {
    sendReadReceipt(sender, [id]);
    message.status = "read";
    markMessageRead(id);
  } else {
    unread.value[key] = true;
  }
}

/** Handles the other side telling us they read our messages. */
function receiveReadReceipt(sender: string, messageIds: string[]) {
  const conversation = messages.value[conversationKey("contact", sender)];
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

// --- Removal --------------------------------------------------------------

/** "1 message" / "4 messages", so the dialog and toast read as sentences. */
function describeMessageCount(count: number): string {
  return count === 1 ? "1 message" : `${count} messages`;
}

/**
 * Step one of removing a contact or leaving a group: look up what would be lost
 * and open the confirmation. Nothing is deleted here.
 */
async function requestRemoval(kind: ConversationKind, id: string, name: string) {
  const command = kind === "contact" ? "count_chat_messages" : "count_group_messages";
  const args = kind === "contact" ? { peerId: id } : { groupId: id };

  let messageCount = -1;
  try {
    messageCount = await invoke<number>(command, args);
  } catch (error) {
    // Worth continuing without the count: the user can still make the decision,
    // and the warning covers the history either way.
    console.error("Could not count stored messages", error);
  }

  pendingRemoval.value = { kind, id, name, messageCount };
}

/** Step two: the user confirmed, so delete. */
async function confirmRemoval() {
  const pending = pendingRemoval.value;
  if (!pending) {
    return;
  }

  pendingRemoval.value = null;

  try {
    let deleted: number;

    if (pending.kind === "contact") {
      deleted = await invoke<number>("delete_contact", { peerId: pending.id });
      await loadContacts();
    } else {
      // Stop listening before forgetting the group, or messages would keep
      // arriving for a conversation we no longer have.
      await invoke("unsubscribe_group", { groupId: pending.id });
      deleted = await invoke<number>("delete_group", { groupId: pending.id });
      await loadGroups();
    }

    // Drop the local copies too, or the conversation would linger on screen
    // until the next restart.
    const key = conversationKey(pending.kind, pending.id);
    delete messages.value[key];
    delete unread.value[key];

    if (selection.value?.kind === pending.kind && selection.value.id === pending.id) {
      selection.value = null;
    }

    const action = pending.kind === "contact" ? "Removed" : "Left";
    notify(
      `${action} ${pending.name} and deleted ${describeMessageCount(deleted)}.`,
      "info",
    );
  } catch (error) {
    notify(`Could not remove ${pending.name}: ${error}`);
  }
}

const removalTitle = computed(() =>
  pendingRemoval.value?.kind === "group" ? "Leave this group?" : "Remove this contact?",
);

const removalConfirmLabel = computed(() =>
  pendingRemoval.value?.kind === "group"
    ? "Leave and delete history"
    : "Remove and delete history",
);

const removalMessage = computed(() => {
  const pending = pendingRemoval.value;
  if (!pending) {
    return "";
  }

  if (pending.kind === "group") {
    return `You'll stop receiving messages from ${pending.name}. The other members carry on without you, and you can be invited back.`;
  }

  return `${pending.name} will be removed from your contacts, and messages from them will be silently dropped until you add them again.`;
});

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
    return "Your entire chat history with this conversation will be permanently deleted. This cannot be undone.";
  }

  if (pending.messageCount === 0) {
    return "There is no chat history here yet, so nothing else will be lost.";
  }

  return `${describeMessageCount(pending.messageCount)} will be permanently deleted along with it. This cannot be undone.`;
});

// --- Startup --------------------------------------------------------------

onMounted(async () => {
  nodeInstance.value = await invoke<string>("get_node_id");
  await loadIdentity();
  await loadContacts();
  await loadGroups();

  await listen<string>("peer-discovered", (event) => {
    activePeers.value.add(event.payload);
  });

  await listen<string>("peer-lost", (event) => {
    activePeers.value.delete(event.payload);
  });

  // Catch up on peers that connected before this window started listening.
  // Deliberately after the listeners are registered, so a peer appearing during
  // startup can't fall into the gap between the two. Adding a peer twice is
  // harmless, since these are held in a Set.
  try {
    const initialPeers = await invoke<string[]>("get_active_peers");
    initialPeers.forEach((peer) => activePeers.value.add(peer));
  } catch (error) {
    console.error("Could not load the current peer list", error);
  }

  await listen<{ sender: string; message: string }>("chat-received", async (event) => {
    const { sender, message } = event.payload;

    const data = parsePayload(message, sender);
    if (!data) {
      return;
    }

    if (data.type === "chat" && data.id && typeof data.text === "string") {
      await receiveChatMessage(sender, data.id, data.text);
    } else if (data.type === "read" && Array.isArray(data.messageIds)) {
      receiveReadReceipt(sender, data.messageIds);
    } else if (
      data.type === "group-invite" &&
      data.groupId &&
      data.groupName &&
      Array.isArray(data.members)
    ) {
      await receiveInvite(sender, data.groupId, data.groupName, data.members);
    }
  });

  await listen<{ group_id: string; sender: string; message: string }>(
    "group-message-received",
    async (event) => {
      const { group_id: groupId, sender, message } = event.payload;

      const data = parsePayload(message, sender);
      if (!data || data.type !== "group-chat" || !data.id || typeof data.text !== "string") {
        return;
      }

      await receiveGroupMessage(
        groupId,
        sender,
        data.id,
        data.text,
        data.groupName,
        data.members,
      );
    },
  );
});

/**
 * Reads a payload off the network.
 *
 * The backend passes payloads through untouched, so anything malformed is
 * dropped here rather than breaking the listener.
 */
function parsePayload(
  message: string,
  sender: string,
): {
  type?: string;
  id?: string;
  text?: string;
  messageIds?: string[];
  groupId?: string;
  groupName?: string;
  members?: string[];
} | null {
  try {
    return JSON.parse(message);
  } catch {
    console.error("Ignoring an unreadable payload from", sender);
    return null;
  }
}
</script>

<template>
  <div class="app">
    <aside class="sidebar">
      <IdentityBar :peer-id="myPeerId" />

      <ContactList
        class="fill"
        :contacts="savedContacts"
        :online-peers="activePeers"
        :unread="contactUnread"
        :selected-peer-id="selectedContact?.peer_id ?? null"
        @select="selectContact"
        @rename="renameContact"
        @remove="(contact) => requestRemoval('contact', contact.peer_id, contact.nickname)"
      />

      <GroupList
        :groups="groups"
        :unread="groupUnread"
        :selected-group-id="selectedGroup?.id ?? null"
        :can-create="savedContacts.length > 0"
        @select="selectGroup"
        @create="creatingGroup = true"
        @leave="(group) => requestRemoval('group', group.id, group.name)"
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
        :key="`contact:${selectedContact.peer_id}`"
        :title="selectedContact.nickname"
        :subtitle="shortPeerId(selectedContact.peer_id)"
        :online="selectedIsOnline"
        :messages="currentMessages"
        :my-peer-id="myPeerId"
        @send="sendMessage"
      />

      <ChatPane
        v-else-if="selectedGroup"
        :key="`group:${selectedGroup.id}`"
        :title="selectedGroup.name"
        :subtitle="`${selectedGroup.members.length} members`"
        :online="null"
        :messages="currentMessages"
        :my-peer-id="myPeerId"
        :sender-labels="groupSenderLabels"
        @send="sendGroupMessage"
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
          Pick a contact or a group, or add a discovered peer to start talking to
          them.
        </p>
      </div>
    </main>

    <NewGroupDialog
      v-if="creatingGroup"
      :contacts="savedContacts"
      @create="createGroup"
      @cancel="creatingGroup = false"
    />

    <ConfirmDialog
      v-if="pendingRemoval"
      :title="removalTitle"
      :message="removalMessage"
      :warning="removalWarning"
      :confirm-label="removalConfirmLabel"
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
   which keeps the sections below it pinned to the bottom. */
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
  z-index: 20;
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
