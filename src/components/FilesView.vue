<script setup lang="ts">
/**
 * Everything this node has sent or received, grouped by who it was with.
 *
 * Grouped rather than one flat list because the question people actually have is
 * "what did Alice send me", not "what happened at 14:32". Grouping is also what
 * makes it obvious that a contact can involve any number of files, which a
 * single chronological list buries.
 *
 * This matters more than it would in most applications, because files arrive
 * without a prompt. This is where you find out what turned up, which is why
 * anything that arrived while you were elsewhere is marked.
 *
 * Files are also picked and sent from here. Picking and sending are deliberately
 * two steps: a batch collects in a tray under the recipient, where it can be
 * looked over and pruned, and only leaves when Send is pressed.
 */
import { computed } from "vue";
import FileRow from "./FileRow.vue";
import GroupTransfers from "./GroupTransfers.vue";
import type { Contact, FileTransfer, Group, PickedFile } from "../types";
import { canSend, describeProblem, describeSize, shortPeerId } from "../types";

const props = defineProps<{
  files: FileTransfer[];
  contacts: Contact[];
  /** Files picked but not yet sent, keyed by who they are going to. */
  staged: Record<string, PickedFile[]>;
  /**
   * Whoever is picked in the sidebar, which is who the toolbar button adds for.
   * The sidebar stays visible in this view precisely so this can be changed.
   */
  selectedPeerId: string | null;
  /**
   * Files that had not been looked at when this view was opened.
   *
   * Held by the parent rather than read from `seen` so the markers stay put
   * while you are reading them, instead of clearing under your eyes.
   */
  newlyArrived: Set<string>;
  /**
   * Who is reachable right now.
   *
   * Shown beside each name because this is where somebody decides to send
   * something, and a transfer to a contact who is not there will sit waiting
   * rather than failing. Worth knowing before pressing Send, not after.
   */
  onlinePeers: Set<string>;
  /** Groups this node is in, so a group's files can be shown under its name. */
  groups: Group[];
}>();

const emit = defineEmits<{
  /** Open the picker and stage whatever is chosen for this peer. */
  add: [peerId: string];
  /** Send everything staged for this peer. */
  send: [peerId: string];
  /** Drop one staged file before it is sent. */
  unstage: [peerId: string, path: string];
  clear: [peerId: string];
  open: [file: FileTransfer];
  reveal: [file: FileTransfer];
  /** Ask again for the rest of one that stopped partway. */
  resume: [file: FileTransfer];
  /** Choose files and send them to everybody in a group. */
  addToGroup: [groupId: string];
  /** Try again everything that failed for one member of a group. */
  resumeMember: [files: FileTransfer[]];
}>();


interface ContactFiles {
  peerId: string;
  name: string;
  /** True when this is somebody we no longer have as a contact. */
  unknown: boolean;
  files: FileTransfer[];
  staged: PickedFile[];
  /** How many staged files could actually go out. */
  sendable: number;
  received: number;
  sent: number;
  bytes: number;
  latest: number;
}

const selectedContact = computed(() =>
  props.contacts.find((contact) => contact.peer_id === props.selectedPeerId) ?? null,
);

const contactFiles = computed<ContactFiles[]>(() => {
  const byPeer = new Map<string, FileTransfer[]>();

  for (const file of props.files) {
    // Group files have sections of their own further down. Without this they
    // would appear in both, which is worse than either.
    if (file.group_id) {
      continue;
    }

    const existing = byPeer.get(file.peer_id);

    if (existing) {
      existing.push(file);
    } else {
      byPeer.set(file.peer_id, [file]);
    }
  }

  // Somebody with a tray but no history still needs somewhere to put it.
  for (const peerId of Object.keys(props.staged)) {
    if (!byPeer.has(peerId)) {
      byPeer.set(peerId, []);
    }
  }

  const gathered: ContactFiles[] = [];

  for (const [peerId, files] of byPeer) {
    // Files outlive contacts on purpose: removing somebody should not delete
    // things off the disk. So a group may belong to nobody we still know.
    const contact = props.contacts.find((candidate) => candidate.peer_id === peerId);
    const staged = props.staged[peerId] ?? [];

    files.sort((a, b) => b.sent_at - a.sent_at);

    gathered.push({
      peerId,
      name: contact?.nickname ?? shortPeerId(peerId),
      unknown: !contact,
      files,
      staged,
      sendable: staged.filter(canSend).length,
      received: files.filter((file) => file.direction === "incoming").length,
      sent: files.filter((file) => file.direction === "outgoing").length,
      bytes: files.reduce((total, file) => total + file.size, 0),
      latest: files[0]?.sent_at ?? 0,
    });
  }

  // A tray is a pending action, so it sorts above finished history rather than
  // being left somewhere down the page to be scrolled past.
  return gathered.sort((a, b) => {
    if ((a.staged.length > 0) !== (b.staged.length > 0)) {
      return a.staged.length > 0 ? -1 : 1;
    }

    return b.latest - a.latest;
  });
});

/**
 * Files belonging to a group, gathered under it.
 *
 * A group we are no longer in is left out. Its files stay on disk, and the
 * per contact list below still shows them, so nothing is hidden by this.
 */
const groupSections = computed(() =>
  props.groups
    .map((group) => ({
      group,
      files: props.files.filter((file) => file.group_id === group.id),
    }))
    .filter((section) => section.files.length > 0)
    .sort(
      (a, b) =>
        Math.max(...b.files.map((f) => f.sent_at)) -
        Math.max(...a.files.map((f) => f.sent_at)),
    ),
);

/** "3 received, 2 sent", leaving out whichever is zero. */
function describeCounts(group: ContactFiles): string {
  const parts = [];

  if (group.received > 0) {
    parts.push(`${group.received} received`);
  }

  if (group.sent > 0) {
    parts.push(`${group.sent} sent`);
  }

  if (parts.length === 0) {
    return "nothing yet";
  }

  return `${parts.join(", ")} · ${describeSize(group.bytes)}`;
}



</script>

<template>
  <section class="files">
    <header class="header">
      <h2 class="title">Files</h2>
      <span v-if="files.length" class="count">{{ files.length }}</span>

      <!-- Adds for whoever is picked in the sidebar. Disabled rather than
           hidden, so the reason it cannot be used is readable. -->
      <button
        class="add"
        :disabled="!selectedContact"
        :title="
          selectedContact
            ? `Choose files to send to ${selectedContact.nickname}`
            : 'Pick a contact on the left to send files to'
        "
        @click="selectedContact && emit('add', selectedContact.peer_id)"
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
          <line x1="12" y1="5" x2="12" y2="19" />
          <line x1="5" y1="12" x2="19" y2="12" />
        </svg>
        {{ selectedContact ? `Add files for ${selectedContact.nickname}` : "Add files" }}
      </button>
    </header>

    <div v-if="contactFiles.length === 0 && groupSections.length === 0" class="empty">
      <svg
        viewBox="0 0 24 24"
        width="38"
        height="38"
        stroke="currentColor"
        stroke-width="1.4"
        fill="none"
      >
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
        <polyline points="14 2 14 8 20 8" />
      </svg>

      <p class="empty-title">Nothing sent or received yet</p>
      <p class="empty-hint">
        Pick a contact on the left, then add files to send them. Anything that
        arrives shows up here too, grouped by who it was with.
      </p>
    </div>

    <div v-else class="groups">
      <!-- Groups first. A send to fifteen people is the thing most likely to
           need looking at, and burying it under one to one history would mean
           scrolling to find out. -->
      <GroupTransfers
        v-for="section in groupSections"
        :key="section.group.id"
        :group="section.group"
        :files="section.files"
        :contacts="contacts"
        :online-peers="onlinePeers"
        :newly-arrived="newlyArrived"
        @add="emit('addToGroup', $event)"
        @resume-member="emit('resumeMember', $event)"
        @open="emit('open', $event)"
        @reveal="emit('reveal', $event)"
        @resume="emit('resume', $event)"
      />

      <section
        v-for="group in contactFiles"
        :key="group.peerId"
        class="group"
        :class="{ selected: group.peerId === selectedPeerId }"
      >
        <header class="group-header">
          <span class="who">
            <span class="who-line">
              <!-- Not shown for somebody who is no longer a contact, where
                   there is nothing to be reachable for. -->
              <span
                v-if="!group.unknown"
                class="presence"
                :class="{ online: onlinePeers.has(group.peerId) }"
                :title="onlinePeers.has(group.peerId) ? 'Online' : 'Offline'"
              />
              <span class="name" :class="{ unknown: group.unknown }" :title="group.peerId">
                {{ group.name }}
              </span>
            </span>
            <span class="summary">{{ describeCounts(group) }}</span>
          </span>

          <!-- Sending from here rather than only from a conversation, since this
               is where somebody is when they are thinking about files. -->
          <button
            v-if="!group.unknown"
            class="group-add"
            :title="`Choose files to send to ${group.name}`"
            @click="emit('add', group.peerId)"
          >
            Add files
          </button>
        </header>

        <!-- The tray: picked, looked over, not yet gone anywhere. -->
        <div v-if="group.staged.length" class="tray">
          <div class="tray-header">
            <span class="tray-title">
              {{ group.staged.length }}
              {{ group.staged.length === 1 ? "file" : "files" }} ready to send
            </span>

            <button class="tray-clear" @click="emit('clear', group.peerId)">Clear</button>
            <button
              class="tray-send"
              :disabled="group.sendable === 0"
              :title="
                group.sendable === 0
                  ? 'None of these can be sent'
                  : `Send to ${group.name}`
              "
              @click="emit('send', group.peerId)"
            >
              Send {{ group.sendable }}
            </button>
          </div>

          <ul class="tray-list">
            <li
              v-for="file in group.staged"
              :key="file.path"
              class="tray-row"
              :class="{ rejected: !canSend(file) }"
            >
              <span class="file-name" :title="file.path">{{ file.name }}</span>
              <span class="tray-meta">
                {{ describeProblem(file) ?? `${describeSize(file.size)} · not sent yet` }}
              </span>

              <button
                class="tray-remove"
                title="Remove from this batch"
                @click="emit('unstage', group.peerId, file.path)"
              >
                <svg
                  viewBox="0 0 24 24"
                  width="13"
                  height="13"
                  stroke="currentColor"
                  stroke-width="2.2"
                  fill="none"
                  stroke-linecap="round"
                >
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </li>
          </ul>
        </div>

        <ul v-if="group.files.length" class="list">
          <FileRow
            v-for="file in group.files"
            :key="file.id"
            :file="file"
            :newly-arrived="newlyArrived"
            @open="emit('open', $event)"
            @reveal="emit('reveal', $event)"
            @resume="emit('resume', $event)"
          />
        </ul>
      </section>
    </div>
  </section>
</template>

<style scoped>
.files {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background-color: var(--bg-sunken);
}

.header {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: none;
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  background-color: var(--bg);
}

.title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
}

.count {
  font-size: 12px;
  color: var(--text-faint);
}

.add {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-left: auto;
  flex: none;
  max-width: 60%;
  padding: 6px 11px;
  border-radius: var(--radius-pill);
  background-color: var(--accent);
  font-size: 12px;
  font-weight: 500;
  color: var(--accent-contrast);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.add:hover:not(:disabled) {
  background-color: var(--accent-hover);
}

.add:disabled {
  background-color: var(--bg-hover);
  color: var(--text-faint);
}

/* Empty state ------------------------------------------------------------ */

.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 4px;
  flex: 1;
  padding: 24px;
  text-align: center;
  color: var(--text-faint);
}

.empty-title {
  margin: 8px 0 0;
  font-weight: 600;
  color: var(--text-muted);
}

.empty-hint {
  margin: 0;
  max-width: 40ch;
  font-size: 13px;
}

/* Groups ----------------------------------------------------------------- */

.groups {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 16px 20px;
}

.group + .group {
  margin-top: 18px;
}

.group-header {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 4px 2px 8px;
}

.who {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.who-line {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}

.presence {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background-color: var(--offline);
}

.presence.online {
  background-color: var(--online);
}

.name {
  overflow: hidden;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Whoever is picked in the sidebar, so choosing a contact while in this view
   visibly points somewhere. */
.group.selected .name {
  color: var(--accent);
}

/* Somebody who is no longer a contact, shown by their id since there is no
   longer a name for them. */
.name.unknown {
  font-family: var(--font-mono);
  font-size: 12px;
  font-weight: 500;
  color: var(--text-muted);
}

.summary {
  font-size: 11px;
  color: var(--text-faint);
}

.group-add {
  margin-left: auto;
  flex: none;
  padding: 5px 10px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  font-size: 12px;
  font-weight: 500;
  color: var(--text);
}

.group-add:hover {
  background-color: var(--bg-hover);
}

/* Tray ------------------------------------------------------------------- */

.tray {
  margin-bottom: 8px;
  border: 1px dashed var(--border-strong);
  border-radius: var(--radius);
  background-color: var(--bg);
}

.tray-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
}

.tray-title {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-muted);
}

.tray-clear {
  flex: none;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  color: var(--text-faint);
}

.tray-clear:hover {
  background-color: var(--bg-hover);
  color: var(--text);
}

.tray-send {
  flex: none;
  padding: 5px 12px;
  border-radius: var(--radius-pill);
  background-color: var(--accent);
  font-size: 12px;
  font-weight: 500;
  color: var(--accent-contrast);
}

.tray-send:hover:not(:disabled) {
  background-color: var(--accent-hover);
}

.tray-send:disabled {
  background-color: var(--bg-hover);
  color: var(--text-faint);
}

.tray-list {
  margin: 0;
  padding: 0;
  list-style: none;
}

.tray-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 12px;
}

.tray-row + .tray-row {
  border-top: 1px solid var(--border);
}

.tray-row .file-name {
  flex: 1;
  min-width: 0;
  font-size: 13px;
}

.tray-meta {
  flex: none;
  font-size: 11px;
  color: var(--text-faint);
}

/* Something that will not go out, said plainly at the point of picking rather
   than as a failure after the fact. */
.tray-row.rejected .file-name {
  color: var(--text-faint);
  text-decoration: line-through;
}

.tray-row.rejected .tray-meta {
  color: var(--danger);
}

.tray-remove {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--text-faint);
}

.tray-remove:hover {
  background-color: var(--bg-hover);
  color: var(--text);
}

/* Rows ------------------------------------------------------------------- */

.list {
  margin: 0;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background-color: var(--bg);
  list-style: none;
  overflow: hidden;
}

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

/* Only on a transfer that stopped partway, where the bytes already written are
   kept and asking again carries on from there. */
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
</style>
