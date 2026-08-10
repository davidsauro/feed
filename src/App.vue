<script setup lang="ts">
/**
 * Application shell.
 *
 * This component owns all state and every call into the Rust backend. The
 * components under it are presentational: they take props and report what the
 * user did, and this file decides what that means.
 *
 * Both kinds of conversation travel the same way: over gossipsub, on a topic
 * only the people involved can name. Nothing requires a connection to the person
 * being written to, so two nodes that could never reach each other directly can
 * still talk as long as both can reach something in the middle.
 */
import { computed, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as pickFiles } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";

import AddMembersDialog from "./components/AddMembersDialog.vue";
import ChatPane from "./components/ChatPane.vue";
import ConfirmDialog from "./components/ConfirmDialog.vue";
import ContactList from "./components/ContactList.vue";
import FilesView from "./components/FilesView.vue";
import GroupList from "./components/GroupList.vue";
import IdentityBar from "./components/IdentityBar.vue";
import NewGroupDialog from "./components/NewGroupDialog.vue";
import PeerList from "./components/PeerList.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import UnlockScreen from "./components/UnlockScreen.vue";
import type {
  ChatMessage,
  Contact,
  FileTransfer,
  Group,
  MessageStatus,
  PickedFile,
  Server,
  ServerStatus,
} from "./types";
import { canSend, shortPeerId } from "./types";

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

/**
 * When each contact was last heard from, in milliseconds since the epoch.
 *
 * Presence for people we hold no connection to. That covers anybody reached
 * through a relay server, where both sides are connected to the server and
 * neither to the other, so a connection cannot answer the question even while
 * messages pass perfectly. They say so instead, every fifteen seconds.
 */
const seenPeers = ref<Record<string, number>>({});

/**
 * Ticks so a contact who stops announcing turns grey without anything else
 * happening.
 *
 * Presence expiring is the one thing here that is not driven by an event: an
 * announcement that does not arrive produces nothing to react to.
 */
const clockTick = ref(Date.now());
const savedContacts = ref<Contact[]>([]);
const groups = ref<Group[]>([]);

/**
 * Conversations and unread flags, keyed by `conversationKey`.
 *
 * Direct chats and groups share these maps, so the key carries the kind as well
 * as the id rather than relying on peer IDs and group IDs never colliding.
 */
const messages = ref<Record<string, ChatMessage[]>>({});

/**
 * Every file this node has sent or received.
 *
 * Kept whole rather than per conversation, because the Files view wants all of
 * them and a conversation only wants a slice, which is cheap to take.
 */
const files = ref<FileTransfer[]>([]);
const unread = ref<Record<string, boolean>>({});

/** The open conversation, or null for the empty state. */
const selection = ref<{ kind: ConversationKind; id: string } | null>(null);

/**
 * What the main pane is showing.
 *
 * The sidebar stays put either way, because picking who to send to is the same
 * act as picking who to talk to. Opening a conversation returns to chats, since
 * that is plainly what selecting one means.
 */
const view = ref<"chats" | "files">("chats");

/**
 * The relay servers this node uses to reach people off the local network.
 *
 * Whether each is reachable is not held here. A server is an ordinary peer once
 * connected, so `activePeers` already answers that.
 */
const servers = ref<Server[]>([]);

/**
 * Measurements for each server: round trip, how long it has been up, and why the
 * last attempt failed.
 *
 * Whether a server is reachable is *not* read from here. That comes from
 * `activePeers`, which the same connection events drive and which updates the
 * moment anything changes. This holds only what a live set cannot: numbers and
 * reasons. Keeping the two apart is what stops a dot and a label disagreeing.
 */
const serverStatus = ref<ServerStatus[]>([]);

/** Set while a manual test is running, which takes a few seconds by design. */
const testingServers = ref(false);

/**
 * Files picked but not yet sent, keyed by who they are going to.
 *
 * Held here rather than in the Files view so a half assembled batch survives
 * flipping back to a conversation and returning. Keyed by peer so batches for
 * two different people can be assembled at once without either being lost.
 */
const staged = ref<Record<string, PickedFile[]>>({});

/**
 * Files that had not been looked at when the Files view was last opened.
 *
 * The database flag is cleared as soon as the view opens, which is right for the
 * badge but would make the markers vanish while they are being read. This keeps
 * them on screen for the visit that revealed them.
 */
const newlyArrived = ref<Set<string>>(new Set());

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
  /**
   * Groups that go with this, which is only ever the case for a contact.
   *
   * Staying in a group with someone we've removed would mean sitting in a
   * conversation with a hole in it: their messages are dropped on arrival, so
   * the group would look complete while a third of it silently never appeared.
   */
  groups: { id: string; name: string; messageCount: number }[];
} | null>(null);

const creatingGroup = ref(false);
const addingMembers = ref(false);
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

/**
 * How far ahead of us a sender's clock may be before we stop believing it.
 *
 * A clock running fast would pin its messages to the bottom of a conversation
 * for as long as the conversation exists. One running slow only misplaces them
 * once, on arrival, so only the fast direction is worth guarding against.
 */
const CLOCK_SKEW_TOLERANCE_MS = 5 * 60 * 1000;

/**
 * Reads the time a message claims it was written.
 *
 * Falls back to now for anything missing or nonsensical, which covers senders
 * running an older version as well as senders being difficult.
 */
function claimedSentAt(claimed: unknown): number {
  const now = Date.now();

  if (typeof claimed !== "number" || !Number.isFinite(claimed) || claimed <= 0) {
    return now;
  }

  return Math.min(claimed, now + CLOCK_SKEW_TOLERANCE_MS);
}

/**
 * Puts a message into a conversation in the position its timestamp calls for.
 *
 * Appending would be wrong now that messages can arrive out of order: the
 * screen has to match what reopening the conversation would show, and that
 * comes back from the database sorted by when each one was written.
 */
function insertMessage(key: string, message: ChatMessage) {
  if (!messages.value[key]) {
    messages.value[key] = [];
  }

  const conversation = messages.value[key];
  const at = conversation.findIndex((existing) => existing.sent_at > message.sent_at);

  if (at === -1) {
    conversation.push(message);
  } else {
    conversation.splice(at, 0, message);
  }
}

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
    (peer) =>
      !savedContacts.value.some((contact) => contact.peer_id === peer) &&
      // A server is a peer, but not somebody you would chat with. Offering to
      // add one as a contact would be offering something that cannot work.
      !servers.value.some((server) => server.peer_id === peer),
  ),
);

/** How many configured servers are reachable right now. */
const connectedServers = computed(
  () => servers.value.filter((server) => activePeers.value.has(server.peer_id)).length,
);

const currentMessages = computed(() => {
  if (!selection.value) {
    return [];
  }

  return messages.value[conversationKey(selection.value.kind, selection.value.id)] ?? [];
});

/** How many transfers are moving right now, in either direction. */
const activeTransfers = computed(
  () =>
    files.value.filter(
      (file) => file.status === "transferring" || file.status === "pending",
    ).length,
);

/**
 * Files that turned up since the Files view was last looked at.
 *
 * A transfer still in flight is not counted, because it has not arrived yet. It
 * raises this once it settles. One that failed does count, since a file that
 * did not make it is worth being told about rather than being swallowed.
 */
const unseenFiles = computed(
  () =>
    files.value.filter(
      (file) =>
        file.direction === "incoming" &&
        !file.seen &&
        (file.status === "complete" || file.status === "failed"),
    ).length,
);

/** The files belonging to whichever conversation is open. */
const currentFiles = computed(() => {
  if (selection.value?.kind !== "contact") {
    return [];
  }

  return files.value.filter((file) => file.peer_id === selection.value?.id);
});

/**
 * How long a contact stays online after their last announcement.
 *
 * Three intervals, so a single one going astray does not blink somebody offline
 * who is sitting right there. The cost is that somebody who really has gone
 * takes up to this long to turn grey, which is the right way round: a wrong
 * "offline" is far more annoying than a late one.
 */
const PRESENCE_TIMEOUT = 45_000;

/**
 * Everybody reachable, by either means.
 *
 * A connection is the better answer where there is one, because it is a fact
 * about right now rather than a claim made a moment ago. Announcements cover
 * everybody else, which is anybody reached through a relay server.
 */
const onlinePeers = computed(() => {
  const online = new Set(activePeers.value);

  for (const [peer, seen] of Object.entries(seenPeers.value)) {
    if (clockTick.value - seen < PRESENCE_TIMEOUT) {
      online.add(peer);
    }
  }

  return online;
});

const selectedIsOnline = computed(
  () => selectedContact.value !== null && onlinePeers.value.has(selectedContact.value.peer_id),
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

/**
 * Adds somebody and starts listening for what they say.
 *
 * Both halves or neither. A contact saved without a subscription looks added and
 * is not: messages to them fail with nobody listening, because listening is what
 * makes the conversation exist. Undoing the save is better than leaving one of
 * those behind for somebody to puzzle over.
 */
async function addContact(peerId: string, nickname: string) {
  const id = peerId.trim();

  try {
    await invoke("save_contact", { peerId: id, nickname });
  } catch (error) {
    notify(`Could not add contact: ${error}`);
    return;
  }

  try {
    await invoke("subscribe_direct", { peerId: id });
  } catch (error) {
    // Put it back the way it was rather than leaving a contact that cannot
    // receive anything.
    try {
      await invoke("delete_contact", { peerId: id });
    } catch (cleanup) {
      console.error("Could not undo a half-finished contact", cleanup);
    }

    notify(`Could not listen for ${nickname}, so they were not added: ${error}`);
    return;
  }

  await loadContacts();
  notify(`Added ${nickname}.`, "info");
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
        await invoke("send_direct", { peerId, message: invite });
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

/**
 * Tells the rest of a group that we're leaving it.
 *
 * Not waited on. Each notice waits to hear that it arrived, and an unreachable
 * member takes the full timeout to give up — nobody should sit watching a
 * spinner to leave a conversation. The notices are already handed to the
 * network, so they still go out after the group is gone from here.
 *
 * A member who is offline for this will go on believing we're in the group. In
 * a group this size that's a wrong number on a screen, and they find out the
 * next time we speak to them, which is never — so it stays wrong. Fixing that
 * properly means versioned membership; it isn't worth it yet.
 */
function announceDeparture(groupId: string, members: string[]) {
  const notice = JSON.stringify({ type: "group-leave", groupId });

  for (const peerId of members) {
    invoke("send_direct", { peerId, message: notice }).catch((error) => {
      console.error(`Could not tell ${peerId} that we left`, error);
    });
  }
}

/** Contacts who could be added to the open group. */
const addableContacts = computed(() => {
  const group = selectedGroup.value;
  if (!group) {
    return [];
  }

  return savedContacts.value.filter((contact) => !group.members.includes(contact.peer_id));
});

/**
 * Adds contacts to the open group, and tells everyone who is now in it.
 *
 * The invite carries the whole membership rather than just the newcomers, so it
 * serves twice over: the people being added learn the group exists, and the
 * people already in it learn who else to encrypt for. Without that second part a
 * new member would only ever hear from whoever added them.
 */
async function addMembers(peerIds: string[]) {
  const group = selectedGroup.value;
  addingMembers.value = false;

  if (!group || peerIds.length === 0) {
    return;
  }

  const members = [...group.members, ...peerIds];

  try {
    await invoke("save_group", { id: group.id, name: group.name, members });
    await loadGroups();
  } catch (error) {
    notify(`Could not add anyone to ${group.name}: ${error}`);
    return;
  }

  const invite = JSON.stringify({
    type: "group-invite",
    groupId: group.id,
    groupName: group.name,
    members,
  });

  const recipients = members.filter((member) => member !== myPeerId.value);
  const results = await Promise.all(
    recipients.map(async (peerId) => {
      try {
        await invoke("send_direct", { peerId, message: invite });
        return null;
      } catch (error) {
        console.error(`Could not tell ${peerId} about the new members`, error);
        return peerId;
      }
    }),
  );

  const unreachable = results.filter((peerId): peerId is string => peerId !== null);
  const added = peerIds.map(displayName).join(", ");

  if (unreachable.length > 0) {
    // Anyone who missed this keeps an older idea of who's in the group, and
    // will neither send to nor hear from the new members until they're told.
    notify(
      `Added ${added}, but could not reach ${unreachable.map(displayName).join(", ")}.`,
    );
  } else {
    notify(`Added ${added} to ${group.name}.`, "info");
  }
}

/** Handles someone announcing that they've left a group we're in. */
async function receiveDeparture(sender: string, groupId: string) {
  const group = groups.value.find((candidate) => candidate.id === groupId);

  if (!group || !group.members.includes(sender)) {
    return;
  }

  const members = group.members.filter((member) => member !== sender);

  try {
    await invoke("save_group", { id: groupId, name: group.name, members });
    await loadGroups();

    notify(`${displayName(sender)} left ${group.name}.`, "info");
  } catch (error) {
    console.error("Could not record that a member left", error);
  }
}

/** Handles an invite from a contact by joining the group they made. */
async function receiveInvite(
  sender: string,
  groupId: string,
  groupName: string,
  members: string[],
) {
  // The same message serves as an invitation and as "here is who is in this
  // group now", so which it is depends on whether we already know the group.
  const alreadyIn = groups.value.some((group) => group.id === groupId);

  try {
    await invoke("save_group", { id: groupId, name: groupName, members });
    await invoke("subscribe_group", { groupId });
    await loadGroups();

    notify(
      alreadyIn
        ? `${displayName(sender)} changed who's in ${groupName}.`
        : `${displayName(sender)} added you to ${groupName}.`,
      "info",
    );
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
  const sentAt = Date.now();

  const key = conversationKey("group", group.id);
  insertMessage(key, {
    id,
    sender: myPeerId.value,
    text,
    status: "sending",
    sent_at: sentAt,
  });

  try {
    await invoke("save_group_message", {
      id,
      groupId: group.id,
      sender: myPeerId.value,
      text,
      status: "sending",
      sentAt,
    });

    await invoke("send_group_message", {
      groupId: group.id,
      message: JSON.stringify({ type: "group-chat", id, text, sentAt }),
    });

    setMessageStatus(key, id, "delivered");
    await invoke("update_message_status", { id, status: "delivered" });
  } catch (error) {
    setMessageStatus(key, id, "failed");
    invoke("update_message_status", { id, status: "failed" }).catch(() => {});
    notify(`Not sent to ${group.name} — ${error}. Click the message to try again.`);
  }
}

/** Handles a group message from a contact. */
async function receiveGroupMessage(
  groupId: string,
  sender: string,
  id: string,
  text: string,
  sentAt: number,
) {
  // Membership deliberately isn't taken from the message. It used to be, so a
  // node with an out-of-date copy would catch up — but it also meant that a
  // member who left was put straight back by the next message from anyone who
  // hadn't heard yet. Membership now changes only when someone is invited or
  // announces they've left.

  // As with a direct message, the database decides whether this is new. Here it
  // also absorbs the duplicate deliveries gossipsub produces on a mesh with
  // loops, which are routine rather than suspicious.
  const stored = await invoke<boolean>("save_group_message", {
    id,
    groupId,
    sender,
    text,
    status: "delivered",
    sentAt,
  });

  if (!stored) {
    return;
  }

  const key = conversationKey("group", groupId);
  insertMessage(key, { id, sender, text, status: "delivered", sent_at: sentAt });

  if (selection.value?.kind !== "group" || selection.value.id !== groupId) {
    unread.value[key] = true;
  }
}

// --- Files ----------------------------------------------------------------

async function loadFiles() {
  try {
    files.value = await invoke<FileTransfer[]>("get_files");
  } catch (error) {
    console.error("Could not load files", error);
  }
}

/** Replaces one file in the list, or adds it if it is new. */
function rememberFile(file: FileTransfer) {
  const at = files.value.findIndex((existing) => existing.id === file.id);

  if (at === -1) {
    files.value.push(file);
  } else {
    files.value[at] = file;
  }
}

/**
 * Picks files and sends them to whoever is open.
 *
 * One offer per file rather than one for the batch, so that a file that fails
 * takes only itself down.
 */
async function attachFiles() {
  if (selectedContact.value) {
    await attachFilesTo(selectedContact.value.peer_id);
  }
}

/**
 * Picks files and sends them to one contact straight away.
 *
 * This is the conversation's behaviour, where attaching something is part of
 * saying it and waiting to press Send again would be odd. The Files view stages
 * instead, since that is a place for assembling a batch.
 */
async function attachFilesTo(peerId: string) {
  const contact = savedContacts.value.find((candidate) => candidate.peer_id === peerId);
  if (!contact) {
    return;
  }

  const paths = await pickPaths(`Send to ${contact.nickname}`);

  for (const path of paths) {
    await sendOneFile(contact, path);
  }
}

/** Opens the picker and returns whatever was chosen, as a list. */
async function pickPaths(title: string): Promise<string[]> {
  const chosen = await pickFiles({ multiple: true, title });

  if (!chosen) {
    return [];
  }

  return Array.isArray(chosen) ? chosen : [chosen];
}

/**
 * Picks files and puts them in a contact's tray without sending anything.
 *
 * Sizes are read here rather than at send time so the tray can say up front that
 * something is too large, instead of accepting it and failing later.
 */
async function stageFilesFor(peerId: string) {
  const contact = savedContacts.value.find((candidate) => candidate.peer_id === peerId);
  if (!contact) {
    return;
  }

  const paths = await pickPaths(`Add files for ${contact.nickname}`);
  if (paths.length === 0) {
    return;
  }

  let picked: PickedFile[];
  try {
    // The peer matters: a file only has a size limit when it has to cross a
    // relay server to get there.
    picked = await invoke<PickedFile[]>("inspect_files", { peerId, paths });
  } catch (error) {
    notify(`Could not read those files: ${error}`);
    return;
  }

  const tray = staged.value[peerId] ?? [];

  // Picking the same file twice should not queue it twice.
  const fresh = picked.filter(
    (file) => !tray.some((existing) => existing.path === file.path),
  );

  staged.value = { ...staged.value, [peerId]: [...tray, ...fresh] };
}

/** Sends everything in a contact's tray, then empties it. */
async function sendStaged(peerId: string) {
  const contact = savedContacts.value.find((candidate) => candidate.peer_id === peerId);
  const tray = staged.value[peerId];

  if (!contact || !tray) {
    return;
  }

  // Emptied first so a second press cannot send the same batch twice while the
  // first one is still going out.
  clearStaged(peerId);

  for (const file of tray.filter(canSend)) {
    await sendOneFile(contact, file.path);
  }
}

/** Drops one file from a tray before it is sent. */
function unstage(peerId: string, path: string) {
  const tray = staged.value[peerId];
  if (!tray) {
    return;
  }

  const left = tray.filter((file) => file.path !== path);

  if (left.length === 0) {
    clearStaged(peerId);
  } else {
    staged.value = { ...staged.value, [peerId]: left };
  }
}

function clearStaged(peerId: string) {
  const rest = { ...staged.value };
  delete rest[peerId];
  staged.value = rest;
}

/**
 * Notes what had not been looked at, then clears the flag.
 *
 * Called on opening the Files view and again whenever something arrives while it
 * is open, so the badge never counts things sitting in plain sight. The ids are
 * kept so the markers stay readable for this visit.
 */
async function markFilesSeen() {
  const arrived = files.value.filter(
    (file) =>
      file.direction === "incoming" &&
      !file.seen &&
      (file.status === "complete" || file.status === "failed"),
  );

  if (arrived.length === 0) {
    return;
  }

  for (const file of arrived) {
    newlyArrived.value.add(file.id);
    file.seen = true;
  }

  try {
    await invoke("mark_files_seen");
  } catch (error) {
    console.error("Could not mark files as seen", error);
  }
}

/**
 * Leaving the Files view forgets the markers, so the next visit only highlights
 * what is new to that visit rather than accumulating for the session.
 */
watch(view, async (now) => {
  if (now === "files") {
    await markFilesSeen();
  } else {
    newlyArrived.value = new Set();
  }
});

/** Where this node can be reached through a relay, if anywhere. */
async function relayedAddresses(): Promise<string[]> {
  try {
    return await invoke<string[]>("get_relayed_addresses");
  } catch (error) {
    console.error("Could not read our relayed addresses", error);
    return [];
  }
}

async function sendOneFile(contact: Contact, path: string) {
  const sentAt = Date.now();

  try {
    // Reads the file, hashes it, and records the offer. Nothing leaves this
    // machine until the recipient asks for it.
    const file = await invoke<FileTransfer>("send_file", {
      peerId: contact.peer_id,
      path,
      sentAt,
    });

    rememberFile(file);

    // Where they should come and get it. Empty on a local network, where
    // they can already reach us, and carried in the offer rather than
    // announced separately so it arrives exactly when it is needed.
    const addresses = await relayedAddresses();

    await invoke("send_direct", {
      peerId: contact.peer_id,
      message: JSON.stringify({
        type: "file-offer",
        id: file.id,
        name: file.name,
        size: file.size,
        hash: file.hash,
        key: file.key,
        addresses,
        sentAt,
      }),
    });
  } catch (error) {
    notify(`Could not send that file: ${error}`);
  }
}

/** Handles a file somebody has offered us, and starts fetching it. */
async function receiveOffer(
  sender: string,
  offer: {
    id: string;
    name: string;
    size: number;
    hash: string;
    key: string;
    addresses?: string[];
  },
  sentAt: number,
) {
  const contact = savedContacts.value.find((saved) => saved.peer_id === sender);

  try {
    const file = await invoke<FileTransfer>("receive_file", {
      offer: {
        peerId: sender,
        nickname: contact?.nickname ?? "",
        id: offer.id,
        name: offer.name,
        size: offer.size,
        hash: offer.hash,
        key: offer.key,
        // Absent from an offer sent by an older node, which simply means they
        // are only reachable directly.
        addresses: offer.addresses ?? [],
        sentAt,
      },
    });

    rememberFile(file);
  } catch (error) {
    notify(`Could not accept ${offer.name}: ${error}`);
  }
}

async function openFile(file: FileTransfer) {
  if (!file.path) {
    return;
  }

  try {
    await openPath(file.path);
  } catch (error) {
    notify(`Could not open ${file.name}: ${error}`);
  }
}

/**
 * Shows a file in whatever the system uses to browse files.
 *
 * On Linux this goes through the desktop portal, which is not running under
 * WSL2 and fails with a D-Bus error about a service nobody provides. Opening
 * the containing folder is the next best thing, and if that also fails the path
 * itself is at least worth showing, since it can be pasted somewhere useful.
 */
async function revealFile(file: FileTransfer) {
  if (!file.path) {
    return;
  }

  try {
    await revealItemInDir(file.path);
    return;
  } catch (error) {
    console.error("Could not reveal the file", error);
  }

  const folder = file.path.slice(0, Math.max(0, file.path.lastIndexOf("/")));

  try {
    await openPath(folder);
  } catch (error) {
    console.error("Could not open the folder either", error);
    notify(`${file.name} is at ${file.path}`, "info");
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
    await invoke("send_direct", { peerId, message: payload });
  } catch (error) {
    // A receipt that doesn't arrive is not worth interrupting the user over;
    // they'll get one next time the conversation is opened.
    console.error("Could not send read receipt", error);
  }
}

/**
 * Opens a direct conversation: loads its history, clears the unread dot, and
 * tells the other side we've read what they sent.
 *
 * Deliberately leaves the chats/files choice alone. Picking a contact while
 * looking at files means "this one", not "take me somewhere else".
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

/**
 * Changes a message's status so the screen actually shows it.
 *
 * Has to go through the conversation rather than through the object that was
 * pushed into it. Pushing stores the object as it is, and Vue only wraps it in
 * something it can watch when the array is read back — so assigning to the
 * original updates the data while leaving the display exactly as it was. That
 * was why a sent group message kept its clock: the status did change, but
 * nothing knew to repaint it, and unlike a direct message no read receipt came
 * along later to force the issue.
 */
function setMessageStatus(key: string, id: string, status: MessageStatus) {
  const message = messages.value[key]?.find((candidate) => candidate.id === id);

  if (message) {
    message.status = status;
  }
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
  const sentAt = Date.now();

  const key = conversationKey("contact", peerId);
  insertMessage(key, {
    id,
    sender: myPeerId.value,
    text,
    status: "sending",
    sent_at: sentAt,
  });

  try {
    await invoke("save_chat_message", {
      id,
      peerId,
      sender: myPeerId.value,
      text,
      status: "sending",
      sentAt,
    });

    // Reaching the conversation is not the same as reaching the person: with
    // anything in the middle, this only means it was carried. It stays at
    // "sending" until they acknowledge it.
    await invoke("send_direct", {
      peerId,
      message: JSON.stringify({ type: "chat", id, text, sentAt }),
    });
  } catch (error) {
    setMessageStatus(key, id, "failed");
    invoke("update_message_status", { id, status: "failed" }).catch(() => {});
    notify(`Not sent to ${contact.nickname} — ${error}. Click the message to try again.`);
  }
}

/**
 * Sends a message that previously failed, keeping its identity and its place.
 *
 * The id is reused so the recipient recognises a repeat and stores it once, and
 * the original send time is kept so the message doesn't jump to the end of the
 * conversation on every attempt.
 */
async function retryMessage(id: string) {
  const conversation = selection.value;
  if (!conversation) {
    return;
  }

  const key = conversationKey(conversation.kind, conversation.id);
  const message = messages.value[key]?.find((candidate) => candidate.id === id);

  if (!message || message.status !== "failed") {
    return;
  }

  setMessageStatus(key, id, "sending");
  invoke("update_message_status", { id, status: "sending" }).catch(() => {});

  const body = { id, text: message.text, sentAt: message.sent_at };

  try {
    if (conversation.kind === "contact") {
      await invoke("send_direct", {
        peerId: conversation.id,
        message: JSON.stringify({ type: "chat", ...body }),
      });
    } else {
      await invoke("send_group_message", {
        groupId: conversation.id,
        message: JSON.stringify({ type: "group-chat", ...body }),
      });

      // A group message is as delivered as it will ever be once it is carried;
      // there is no single recipient to acknowledge it.
      setMessageStatus(key, id, "delivered");
      await invoke("update_message_status", { id, status: "delivered" });
    }
  } catch (error) {
    setMessageStatus(key, id, "failed");
    invoke("update_message_status", { id, status: "failed" }).catch(() => {});
    notify(`Still could not send: ${error}`);
  }
}

/**
 * Tells a contact their message arrived, which is what turns their clock into a
 * tick.
 *
 * Sent by the receiving node, so it means the message is stored here — a
 * stronger claim than anything the network layer can make on its own.
 */
async function sendAck(peerId: string, id: string) {
  try {
    await invoke("send_direct", {
      peerId,
      message: JSON.stringify({ type: "ack", id }),
    });
  } catch (error) {
    console.error(`Could not acknowledge ${id}`, error);
  }
}

/** Handles a contact confirming they have one of our messages. */
function receiveAck(sender: string, id: string) {
  const key = conversationKey("contact", sender);
  const message = messages.value[key]?.find((candidate) => candidate.id === id);

  // A read receipt says more than an acknowledgement, so never go backwards.
  if (message && message.status !== "sending") {
    return;
  }

  setMessageStatus(key, id, "delivered");
  invoke("update_message_status", { id, status: "delivered" }).catch((error) => {
    console.error(`Could not record the acknowledgement of ${id}`, error);
  });
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
async function receiveChatMessage(sender: string, id: string, text: string, sentAt: number) {
  const stored = await invoke<boolean>("save_chat_message", {
    id,
    peerId: sender,
    sender,
    text,
    status: "delivered",
    sentAt,
  });

  // Acknowledged whether or not it was new. A repeat usually means the sender is
  // retrying because our first acknowledgement went missing, and staying silent
  // would leave them believing a message they successfully sent had failed.
  sendAck(sender, id);

  if (!stored) {
    console.warn(`Ignored a message from ${sender} reusing the id ${id}`);
    return;
  }

  const key = conversationKey("contact", sender);
  insertMessage(key, { id, sender, text, status: "delivered", sent_at: sentAt });

  // If their conversation is already on screen, it's read the moment it lands.
  if (selection.value?.kind === "contact" && selection.value.id === sender) {
    sendReadReceipt(sender, [id]);
    setMessageStatus(key, id, "read");
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

  // Removing a contact means leaving the groups they're in, so those have to be
  // counted up before asking, not sprung afterwards.
  const shared = kind === "contact" ? groupsContaining(id) : [];
  const sharedGroups = await Promise.all(
    shared.map(async (group) => ({
      id: group.id,
      name: group.name,
      messageCount: await countGroupMessages(group.id),
    })),
  );

  pendingRemoval.value = { kind, id, name, messageCount, groups: sharedGroups };
}

/** The groups a peer is a member of. */
function groupsContaining(peerId: string): Group[] {
  return groups.value.filter((group) => group.members.includes(peerId));
}

/** How many messages a group holds, or 0 if that can't be established. */
async function countGroupMessages(groupId: string): Promise<number> {
  try {
    return await invoke<number>("count_group_messages", { groupId });
  } catch (error) {
    console.error(`Could not count the messages in ${groupId}`, error);
    return 0;
  }
}

/** Step two: the user confirmed, so delete. */
async function confirmRemoval() {
  const pending = pendingRemoval.value;
  if (!pending) {
    return;
  }

  pendingRemoval.value = null;

  try {
    let deleted = 0;

    if (pending.kind === "contact") {
      // Groups first, so their departure notices are on their way before
      // anything else changes. Each one needs the member list that is about to
      // be deleted with it.
      for (const group of pending.groups) {
        deleted += await leaveGroup(group.id);
      }

      deleted += await invoke<number>("delete_contact", { peerId: pending.id });

      // Forget when they were last heard from as well, or adding them back
      // would show them online on the strength of an announcement made before
      // they were removed.
      const { [pending.id]: _forgotten, ...stillKnown } = seenPeers.value;
      seenPeers.value = stillKnown;
      await invoke("unsubscribe_direct", { peerId: pending.id });
      await loadContacts();
      await loadGroups();
    } else {
      deleted = await leaveGroup(pending.id);
      await loadGroups();
    }

    // Drop the local copies too, or the conversation would linger on screen
    // until the next restart.
    forgetConversation(pending.kind, pending.id);

    notify(describeRemoval(pending, deleted), "info");
  } catch (error) {
    notify(`Could not remove ${pending.name}: ${error}`);
  }
}

/**
 * Leaves one group: tells the others, stops listening, and deletes it here.
 *
 * Returns how many messages went with it.
 */
async function leaveGroup(groupId: string): Promise<number> {
  // Tell the others while we still know who they are.
  const group = groups.value.find((candidate) => candidate.id === groupId);
  const others = (group?.members ?? []).filter((member) => member !== myPeerId.value);
  announceDeparture(groupId, others);

  // Stop listening before forgetting the group, or messages would keep arriving
  // for a conversation we no longer have.
  await invoke("unsubscribe_group", { groupId });
  const deleted = await invoke<number>("delete_group", { groupId });

  forgetConversation("group", groupId);

  return deleted;
}

/** Drops a conversation from the screen and from what's held in memory. */
function forgetConversation(kind: ConversationKind, id: string) {
  const key = conversationKey(kind, id);
  delete messages.value[key];
  delete unread.value[key];

  if (selection.value?.kind === kind && selection.value.id === id) {
    selection.value = null;
  }
}

/** Says what just happened, including the groups if any went with it. */
function describeRemoval(
  pending: NonNullable<typeof pendingRemoval.value>,
  deleted: number,
): string {
  const action = pending.kind === "contact" ? "Removed" : "Left";
  const groups =
    pending.groups.length > 0
      ? ` and left ${pending.groups.length === 1 ? "1 group" : `${pending.groups.length} groups`}`
      : "";

  return `${action} ${pending.name}${groups}, deleting ${describeMessageCount(deleted)}.`;
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

  if (pending.groups.length > 0) {
    return `${pending.name} will be removed from your contacts, and messages from them will be silently dropped until you add them again — including in groups, which is why you'll also leave the ${pending.groups.length === 1 ? "group" : "groups"} you share with them rather than sit in a conversation missing everything they say.`;
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

  // Naming the groups matters: they hold conversations with other people, who
  // have nothing to do with the contact being removed.
  const groupNames = pending.groups.map((group) => group.name).join(", ");
  const groupMessages = pending.groups.reduce(
    (total, group) => total + group.messageCount,
    0,
  );

  const alsoLeaving =
    pending.groups.length > 0
      ? ` You will also leave ${groupNames}, deleting a further ${describeMessageCount(groupMessages)}.`
      : "";

  if (pending.messageCount < 0) {
    return `Your entire chat history with this conversation will be permanently deleted.${alsoLeaving} This cannot be undone.`;
  }

  if (pending.messageCount === 0 && pending.groups.length === 0) {
    return "There is no chat history here yet, so nothing else will be lost.";
  }

  return `${describeMessageCount(pending.messageCount)} will be permanently deleted along with it.${alsoLeaving} This cannot be undone.`;
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

  // Anything left mid-flight by the last run will never be acknowledged now.
  try {
    await invoke("fail_stale_sends");
  } catch (error) {
    console.error("Could not settle unfinished sends", error);
  }

  await loadIdentity();
  await loadNames();
  await loadContacts();
  await subscribeToContacts();
  await connectToServers();
  await loadFiles();
  await loadGroups();
  await subscribeToSavedGroups();

  await listen<string>("peer-discovered", (event) => {
    activePeers.value.add(event.payload);
  });

  await listen<string>("peer-lost", (event) => {
    activePeers.value.delete(event.payload);
  });

  // Progress arrives per chunk, so this updates one number rather than
  // reloading everything.
  await listen<{ transfer_id: string; transferred: number }>("file-progress", (event) => {
    const file = files.value.find((candidate) => candidate.id === event.payload.transfer_id);

    if (file) {
      file.transferred = event.payload.transferred;
      file.status = "transferring";
    }
  });

  // A transfer finishing or failing changes more than a number, so the record is
  // read back rather than guessed at.
  await listen<string>("file-changed", async () => {
    await loadFiles();

    // Something that lands while this view is open has been seen by definition.
    if (view.value === "files") {
      await markFilesSeen();
    }
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

    // Anything arriving from somebody is proof they are there, whether or not it
    // was sent to say so. Recorded before the payload is routed so a message
    // counts as much as an announcement does.
    seenPeers.value = { ...seenPeers.value, [sender]: Date.now() };

    if (data.type === "presence") {
      // Nothing further. Being here was the whole message.
      return;
    }

    if (data.type === "chat" && data.id && typeof data.text === "string") {
      await receiveChatMessage(sender, data.id, data.text, claimedSentAt(data.sentAt));
    } else if (data.type === "ack" && data.id) {
      receiveAck(sender, data.id);
    } else if (data.type === "read" && Array.isArray(data.messageIds)) {
      receiveReadReceipt(sender, data.messageIds);
    } else if (
      data.type === "group-invite" &&
      data.groupId &&
      data.groupName &&
      Array.isArray(data.members)
    ) {
      await receiveInvite(sender, data.groupId, data.groupName, data.members);
    } else if (
      data.type === "file-offer" &&
      data.id &&
      data.name &&
      typeof data.size === "number" &&
      data.hash &&
      data.key
    ) {
      await receiveOffer(
        sender,
        {
          id: data.id,
          name: data.name,
          size: data.size,
          hash: data.hash,
          key: data.key,
          // Where to go and get it. Without these there is no way to reach
          // somebody who is not on this network, and the offer is the only
          // place they are carried.
          addresses: Array.isArray(data.addresses) ? data.addresses : [],
        },
        claimedSentAt(data.sentAt),
      );
    } else if (data.type === "group-leave" && data.groupId) {
      await receiveDeparture(sender, data.groupId);
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
        claimedSentAt(data.sentAt),
      );
    },
  );
}

// --- Servers --------------------------------------------------------------

/**
 * Loads the configured servers and starts connecting to them.
 *
 * Same timing as subscriptions and for the same reason: on an encrypted node the
 * database cannot be read until the passphrase has been entered, which is long
 * after the network task starts. Connecting is not waited on. A server that is
 * down is retried by the backend on a timer, so nothing here should block
 * startup on one being reachable.
 */
async function connectToServers() {
  try {
    servers.value = await invoke<Server[]>("connect_to_saved_servers");
  } catch (error) {
    console.error("Could not connect to the saved servers", error);
  }
}

/**
 * Saves a server and starts connecting to it.
 *
 * The address is checked by the backend before it is stored, so a typo comes
 * back here as a message rather than becoming a connection that silently never
 * succeeds.
 */
async function addServer(address: string) {
  try {
    const server = await invoke<Server>("add_server", { address });

    // Adding one already in the list should not list it twice.
    if (!servers.value.some((existing) => existing.address === server.address)) {
      servers.value.push(server);
    }
  } catch (error) {
    notify(`Could not add that server: ${error}`);
  }
}

async function removeServer(address: string) {
  try {
    await invoke("remove_server", { address });
    servers.value = servers.value.filter((server) => server.address !== address);
    serverStatus.value = serverStatus.value.filter((status) => status.address !== address);
  } catch (error) {
    notify(`Could not remove that server: ${error}`);
  }
}

/** Reads the current measurements without disturbing anything. */
async function loadServerStatus() {
  try {
    serverStatus.value = await invoke<ServerStatus[]>("get_server_status");
  } catch (error) {
    console.error("Could not read the server status", error);
  }
}

/**
 * Checks every server and reports what came back.
 *
 * This dials the ones that are not connected rather than only reporting what is
 * already known, which is what makes it a test rather than a readout. It takes a
 * few seconds on purpose: a dial is not instant, and answering sooner would only
 * describe the state we started in.
 */
async function testServers() {
  if (testingServers.value) {
    return;
  }

  testingServers.value = true;

  try {
    serverStatus.value = await invoke<ServerStatus[]>("test_servers");
  } catch (error) {
    notify(`Could not test the servers: ${error}`);
  } finally {
    testingServers.value = false;
  }
}

/**
 * Starts listening to each contact's conversation.
 *
 * Only contacts, which is what keeps strangers out: anyone can work out the name
 * of the conversation they would have with us, but nothing published there
 * reaches a node that isn't listening for it.
 */
async function subscribeToContacts() {
  for (const contact of savedContacts.value) {
    try {
      await invoke("subscribe_direct", { peerId: contact.peer_id });
    } catch (error) {
      console.error(`Could not listen for ${contact.nickname}`, error);
    }
  }
}

/**
 * Subscribes to the groups we're already in.
 *
 * The backend can't do this at startup: with encryption on, the database is
 * unreadable until the passphrase arrives, which is long after the network task
 * begins.
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
  sentAt?: number;
  name?: string;
  size?: number;
  hash?: string;
  key?: string;
  /** Where a file's sender says they can be reached. Offers only. */
  addresses?: string[];
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
      <IdentityBar :peer-id="myPeerId" :name="myDisplayName" />

      <nav class="views">
        <button
          class="view"
          :class="{ active: view === 'chats' }"
          @click="view = 'chats'"
        >
          Chats
        </button>
        <button
          class="view"
          :class="{ active: view === 'files' }"
          @click="view = 'files'"
        >
          Files
          <!-- Two different things, and what arrived is the one worth
               interrupting for. A running total of every file ever would sit
               there saying nothing, so neither badge is that. -->
          <span v-if="unseenFiles > 0" class="badge arrived">{{ unseenFiles }}</span>
          <span v-else-if="activeTransfers > 0" class="badge">{{ activeTransfers }}</span>
        </button>
      </nav>

      <ContactList
        class="fill"
        :contacts="savedContacts"
        :online-peers="onlinePeers"
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

        <!-- Only once a server is configured. On a local network there is
             nothing here to report, and an indicator saying so would just be
             something else to read. -->
        <button
          v-if="servers.length"
          class="server-badge"
          :class="{ connected: connectedServers > 0 }"
          :title="
            connectedServers > 0
              ? `Connected to ${connectedServers} of ${servers.length} servers`
              : 'Not connected to any server'
          "
          @click="settingsOpen = true"
        >
          <span class="dot" />
          {{ connectedServers }}/{{ servers.length }}
        </button>

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
      <FilesView
        v-if="view === 'files'"
        :files="files"
        :contacts="savedContacts"
        :staged="staged"
        :selected-peer-id="selectedContact?.peer_id ?? null"
        :newly-arrived="newlyArrived"
        @add="stageFilesFor"
        @send="sendStaged"
        @unstage="unstage"
        @clear="clearStaged"
        @open="openFile"
        @reveal="revealFile"
      />

      <ChatPane
        v-else-if="selectedContact"
        :key="`contact:${selectedContact.peer_id}`"
        :title="selectedContact.nickname"
        :subtitle="shortPeerId(selectedContact.peer_id)"
        :online="selectedIsOnline"
        :messages="currentMessages"
        :files="currentFiles"
        :my-peer-id="myPeerId"
        @send="sendMessage"
        @retry="retryMessage"
        @attach="attachFiles"
        @open-file="openFile"
        @reveal-file="revealFile"
      />

      <ChatPane
        v-else-if="selectedGroup"
        :key="`group:${selectedGroup.id}`"
        :title="selectedGroup.name"
        :subtitle="`${selectedGroup.members.length} members`"
        :online="null"
        :messages="currentMessages"
        :files="[]"
        :my-peer-id="myPeerId"
        :sender-labels="groupSenderLabels"
        :can-add-members="true"
        @send="sendGroupMessage"
        @retry="retryMessage"
        @add-members="addingMembers = true"
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
          them. Files you have sent and received are under Files.
        </p>
      </div>
    </main>

    <SettingsDialog
      v-if="settingsOpen"
      :encryption-enabled="encryption.enabled"
      :display-name="myDisplayName"
      :servers="servers"
      :server-status="serverStatus"
      :online-peers="activePeers"
      :testing-servers="testingServers"
      @enable="enableEncryption"
      @disable="disableEncryption"
      @rename="changeDisplayName"
      @add-server="addServer"
      @remove-server="removeServer"
      @test-servers="testServers"
      @refresh-servers="loadServerStatus"
      @close="settingsOpen = false"
    />

    <AddMembersDialog
      v-if="addingMembers && selectedGroup"
      :group-name="selectedGroup.name"
      :candidates="addableContacts"
      @add="addMembers"
      @cancel="addingMembers = false"
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

.views {
  display: flex;
  gap: 4px;
  flex: none;
  padding: 8px 8px 0;
}

.view {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  flex: 1;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  font-weight: 500;
  color: var(--text-muted);
}

.view:hover {
  background-color: var(--bg-hover);
  color: var(--text);
}

.view.active {
  background-color: var(--bg-active);
  color: var(--accent);
}

/* Transfers in flight: worth showing, not worth a colour that demands
   attention, since it clears itself. */
.badge {
  padding: 0 5px;
  border-radius: var(--radius-pill);
  background-color: var(--border-strong);
  color: var(--text);
  font-size: 10px;
  font-weight: 600;
}

/* Files that arrived while you were elsewhere. Nothing else tells you. */
.badge.arrived {
  background-color: var(--accent);
  color: var(--accent-contrast);
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

/* How many configured servers are actually reachable. Grey until at least one
   is, since "configured" and "working" are different things and only the
   second one means you can reach anybody off this network. */
.server-badge {
  display: flex;
  align-items: center;
  gap: 5px;
  margin-left: 8px;
  flex: none;
  padding: 2px 7px;
  border-radius: var(--radius-pill);
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-faint);
}

.server-badge:hover {
  background-color: var(--bg-hover);
}

.server-badge .dot {
  background-color: var(--offline);
}

.server-badge.connected .dot {
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
