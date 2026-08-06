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
import SettingsDialog from "./components/SettingsDialog.vue";
import UnlockScreen from "./components/UnlockScreen.vue";
import type { ChatMessage, Contact, Group } from "./types";
import { shortPeerId } from "./types";

/** Which kind of thing a conversation is with. */
type ConversationKind = "contact" | "group";

// --- State ----------------------------------------------------------------

const myPeerId = ref("");
const nodeInstance = ref("…");

/** The name this node asks others to call it. */
const myDisplayName = ref("");

/**
 * What other nodes call themselves, by peer id.
 *
 * Only a suggestion: it fills in the nickname box when adding a contact, and
 * whatever the user settles on is theirs and is never overwritten by this.
 */
const peerNames = ref<Record<string, string>>({});

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
const settingsOpen = ref(false);

/**
 * Whether the database is encrypted, and whether it can be read yet.
 *
 * Until this says unlocked, nothing else may touch the database, so the app
 * shows the unlock screen and loads nothing. Assumed locked until the backend
 * says otherwise, so a slow answer can't flash the UI on screen first.
 */
const encryption = ref({ enabled: false, unlocked: false });
const unlockError = ref("");
const unlocking = ref(false);

/**
 * Whether the status above has been answered yet.
 *
 * Nothing renders until it has. Without this the window would show the unlock
 * screen for a moment on every ordinary startup, since "locked" is what we
 * assume before the backend replies.
 */
const encryptionChecked = ref(false);

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

/** Loads this node's own name, and the names other nodes have announced. */
async function loadNames() {
  try {
    myDisplayName.value = await invoke<string>("get_display_name");
  } catch (error) {
    console.error("Could not load this node's name", error);
  }

  try {
    peerNames.value = await invoke<Record<string, string>>("get_peer_names");
  } catch (error) {
    console.error("Could not load the names of other nodes", error);
  }
}

/** Changes this node's name and tells everyone currently connected. */
async function changeDisplayName(name: string) {
  try {
    await invoke("set_display_name", { name });
    myDisplayName.value = name;

    notify(
      name ? `Other nodes will see you as ${name}.` : "Your name has been cleared.",
      "info",
    );
  } catch (error) {
    notify(`Could not save your name: ${error}`);
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

  // Open the group straight away. Each invite now waits to hear whether it
  // actually arrived, and an unreachable member takes the full timeout to give
  // up, so waiting for all of them before showing the group would leave the user
  // staring at nothing.
  selection.value = { kind: "group", id };

  // Sent together rather than one after another, so one unreachable member
  // doesn't delay the invitations to everyone after them.
  const results = await Promise.all(
    memberPeerIds.map(async (peerId) => {
      try {
        await invoke("send_message", { peerId, message: invite });
        return null;
      } catch (error) {
        console.error(`Could not invite ${peerId}`, error);
        return peerId;
      }
    }),
  );

  const uninvited = results.filter((peerId): peerId is string => peerId !== null);

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

  // As with a direct message, the database decides whether this is new. Here it
  // also absorbs the duplicate deliveries gossipsub produces on a mesh with
  // loops, which are routine rather than suspicious.
  const stored = await invoke<boolean>("save_group_message", {
    id,
    groupId,
    sender,
    text,
    status: "delivered",
  });

  if (!stored) {
    return;
  }

  const key = conversationKey("group", groupId);
  if (!messages.value[key]) {
    messages.value[key] = [];
  }
  messages.value[key].push({ id, sender, text, status: "delivered" });

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

/**
 * Handles a direct chat message from a contact.
 *
 * Storing it comes first, and whether the database accepted it decides whether
 * it is shown. Ids are chosen by whoever sent the message, and a contact knows
 * the ids of messages we sent them — we include them so they can be
 * acknowledged — so a reused id is either a duplicate delivery or an attempt to
 * talk over a message we already have. The stored one wins in both cases, and
 * asking the database is what makes that true even for a conversation we
 * haven't loaded.
 */
async function receiveChatMessage(sender: string, id: string, text: string) {
  const stored = await invoke<boolean>("save_chat_message", {
    id,
    peerId: sender,
    sender,
    text,
    status: "delivered",
  });

  if (!stored) {
    console.warn(`Ignored a message from ${sender} reusing the id ${id}`);
    return;
  }

  const key = conversationKey("contact", sender);
  const message: ChatMessage = { id, sender, text, status: "delivered" };

  if (!messages.value[key]) {
    messages.value[key] = [];
  }
  messages.value[key].push(message);

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

// --- Encryption at rest ---------------------------------------------------

async function refreshEncryptionStatus() {
  try {
    encryption.value = await invoke<{ enabled: boolean; unlocked: boolean }>(
      "get_encryption_status",
    );
  } catch (error) {
    // Assume the worst rather than starting a session against a database we
    // might not be able to read.
    console.error("Could not read the encryption status", error);
    encryption.value = { enabled: true, unlocked: false };
  }
}

/** Tries a passphrase, and starts the session if it works. */
async function unlockDatabase(passphrase: string) {
  unlocking.value = true;
  unlockError.value = "";

  try {
    await invoke("unlock_database", { passphrase });
    encryption.value = { enabled: true, unlocked: true };
    await startSession();
  } catch (error) {
    unlockError.value = `${error}`;
  } finally {
    unlocking.value = false;
  }
}

/** Throws away the unreadable database and starts fresh. */
async function resetEverything() {
  unlocking.value = true;

  try {
    await invoke("reset_all_data");
    encryption.value = { enabled: false, unlocked: true };
    unlockError.value = "";
    await startSession();
    notify("All data deleted. Starting fresh.", "info");
  } catch (error) {
    unlockError.value = `Could not delete the data: ${error}`;
  } finally {
    unlocking.value = false;
  }
}

async function enableEncryption(passphrase: string) {
  try {
    await invoke("enable_encryption", { passphrase });
    await refreshEncryptionStatus();
    settingsOpen.value = false;
    notify("Your data is now encrypted on this device.", "info");
  } catch (error) {
    settingsOpen.value = false;
    notify(`Could not encrypt your data: ${error}`);
  }
}

async function disableEncryption() {
  try {
    await invoke("disable_encryption");
    await refreshEncryptionStatus();
    settingsOpen.value = false;
    notify("Encryption turned off. Your data is stored unencrypted.", "info");
  } catch (error) {
    settingsOpen.value = false;
    notify(`Could not turn off encryption: ${error}`);
  }
}

// --- Startup --------------------------------------------------------------

/**
 * Loads everything and starts listening.
 *
 * Split out from mounting because with encryption on none of it can happen
 * until the user has entered their passphrase.
 */
async function startSession() {
  // Starts listening, answering mDNS, and connecting. Deliberately not done at
  // launch: a locked node stays off the network entirely rather than appearing
  // online to everyone while dropping every message sent to it.
  try {
    await invoke("start_network");
  } catch (error) {
    notify(`Could not start networking: ${error}`);
  }

  await loadIdentity();
  await loadNames();
  await loadContacts();
  await loadGroups();
  await subscribeToSavedGroups();

  await listen<string>("peer-discovered", (event) => {
    activePeers.value.add(event.payload);
  });

  await listen<string>("peer-lost", (event) => {
    activePeers.value.delete(event.payload);
  });

  await listen<{ peer_id: string; name: string }>("peer-name", (event) => {
    peerNames.value[event.payload.peer_id] = event.payload.name;
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
}

/**
 * Subscribes to the groups we're already in.
 *
 * The backend can't do this at startup any more: with encryption on, the
 * database is unreadable until the passphrase arrives, which is long after the
 * network task begins.
 */
async function subscribeToSavedGroups() {
  for (const group of groups.value) {
    try {
      await invoke("subscribe_group", { groupId: group.id });
    } catch (error) {
      console.error(`Could not subscribe to ${group.name}`, error);
    }
  }
}

onMounted(async () => {
  nodeInstance.value = await invoke<string>("get_node_id");

  // Nothing else may touch the database until we know it can be read.
  await refreshEncryptionStatus();
  encryptionChecked.value = true;

  if (encryption.value.unlocked) {
    await startSession();
  }
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
  <!-- Nothing at all until we know whether the database can be read, so the
       window never shows one screen and then replaces it with the other. -->
  <template v-if="encryptionChecked">
    <!-- Nothing behind this: with no passphrase there is nothing to show. -->
    <UnlockScreen
      v-if="!encryption.unlocked"
      :error="unlockError"
      :busy="unlocking"
      @unlock="unlockDatabase"
      @reset="resetEverything"
    />

    <div v-else class="app">
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

      <PeerList :peers="unregisteredPeers" :names="peerNames" @add="addContact" />

      <footer class="footer">
        <span class="node-badge">
          <span class="dot" />
          Node {{ nodeInstance }}
        </span>

        <button class="settings-button" title="Settings" @click="settingsOpen = true">
          <svg
            viewBox="0 0 24 24"
            width="15"
            height="15"
            stroke="currentColor"
            stroke-width="2"
            fill="none"
          >
            <circle cx="12" cy="12" r="3" />
            <path
              d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
            />
          </svg>
        </button>
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

    <SettingsDialog
      v-if="settingsOpen"
      :encryption-enabled="encryption.enabled"
      :display-name="myDisplayName"
      @enable="enableEncryption"
      @disable="disableEncryption"
      @rename="changeDisplayName"
      @close="settingsOpen = false"
    />

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

.settings-button {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 26px;
  height: 26px;
  border-radius: var(--radius-sm);
  color: var(--text-faint);
}

.settings-button:hover {
  background-color: var(--bg-hover);
  color: var(--text);
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
