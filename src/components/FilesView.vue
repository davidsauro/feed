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
import type { Contact, FileTransfer, PickedFile } from "../types";
import { canSend, describeProblem, describeSize, describeWhen, shortPeerId } from "../types";

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
}>();

/**
 * Whether asking again could finish this one off.
 *
 * Only ever the receiving side. The receiver is the one that knows how much it
 * already has, and the sender serves whatever chunk it is asked for, so there
 * is nothing for the sender to retry.
 */
const canResume = (file: FileTransfer) =>
  file.direction === "incoming" && file.status === "failed";

interface Group {
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

const groups = computed<Group[]>(() => {
  const byPeer = new Map<string, FileTransfer[]>();

  for (const file of props.files) {
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

  const groups: Group[] = [];

  for (const [peerId, files] of byPeer) {
    // Files outlive contacts on purpose: removing somebody should not delete
    // things off the disk. So a group may belong to nobody we still know.
    const contact = props.contacts.find((candidate) => candidate.peer_id === peerId);
    const staged = props.staged[peerId] ?? [];

    files.sort((a, b) => b.sent_at - a.sent_at);

    groups.push({
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
  return groups.sort((a, b) => {
    if ((a.staged.length > 0) !== (b.staged.length > 0)) {
      return a.staged.length > 0 ? -1 : 1;
    }

    return b.latest - a.latest;
  });
});

/** "3 received, 2 sent", leaving out whichever is zero. */
function describeCounts(group: Group): string {
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

/** What one row says about where a file got to. */
function describeStatus(file: FileTransfer): string {
  const incoming = file.direction === "incoming";

  switch (file.status) {
    case "complete":
      return incoming ? "received" : "sent";
    case "transferring":
      return incoming ? "receiving" : "sending";
    case "pending":
      // The backend says what it is waiting on, which for a peer who is not
      // answering is the difference between a progress report and a frozen row.
      return file.error ?? "starting";
    case "offered":
      return "waiting for them";
    case "failed":
      return file.error ?? "failed";
    default:
      return file.status;
  }
}

function progress(file: FileTransfer): number {
  if (file.size === 0) {
    return 100;
  }

  return Math.min(100, Math.round((file.transferred / file.size) * 100));
}

const inFlight = (file: FileTransfer) =>
  file.status === "transferring" || file.status === "pending";
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

    <div v-if="groups.length === 0" class="empty">
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
      <section
        v-for="group in groups"
        :key="group.peerId"
        class="group"
        :class="{ selected: group.peerId === selectedPeerId }"
      >
        <header class="group-header">
          <span class="who">
            <span class="name" :class="{ unknown: group.unknown }" :title="group.peerId">
              {{ group.name }}
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
          <li v-for="file in group.files" :key="file.id" class="row" :class="file.status">
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
              <span class="meta">
                {{ describeSize(file.size) }} · {{ describeStatus(file) }} ·
                {{ describeWhen(file.sent_at) }}
              </span>

              <span v-if="inFlight(file)" class="progress">
                <span class="bar" :style="{ width: `${progress(file)}%` }" />
              </span>
            </span>

            <span v-if="canResume(file)" class="actions">
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
