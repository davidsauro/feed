<script setup lang="ts">
/**
 * One group's files, gathered by the person they were with.
 *
 * A group send is one transfer per member, so fifteen people and five files is
 * seventy five transfers. Listing those would bury the one thing anybody wants
 * to know, which is whether it got there. So each member is a single line
 * saying how far along they are and whether anything failed, and the files
 * themselves are behind it, opened only when the summary says something is
 * wrong.
 *
 * A member is summarised across every file in this group rather than the most
 * recent send. One scope is easier to trust than two, and a failure from
 * yesterday is still a failure.
 */
import { computed, ref } from "vue";
import FileRow from "./FileRow.vue";
import type { Contact, FileTransfer, Group } from "../types";
import { describeSize, shortPeerId } from "../types";

const props = defineProps<{
  group: Group;
  /** Every file belonging to this group, in either direction. */
  files: FileTransfer[];
  contacts: Contact[];
  onlinePeers: Set<string>;
  newlyArrived: Set<string>;
}>();

const emit = defineEmits<{
  /** Choose files and send them to everybody in this group. */
  add: [groupId: string];
  /** Try again everything that failed for one member. */
  resumeMember: [files: FileTransfer[]];
  open: [file: FileTransfer];
  reveal: [file: FileTransfer];
  resume: [file: FileTransfer];
}>();

interface Member {
  peerId: string;
  name: string;
  files: FileTransfer[];
  done: number;
  total: number;
  failed: FileTransfer[];
  /** Whichever file is moving right now, if any. */
  active: FileTransfer | null;
  bytes: number;
  sent: number;
}

/** Which members are opened up. Closed is the default, which is the point. */
const opened = ref<Set<string>>(new Set());

function toggle(peerId: string) {
  const next = new Set(opened.value);
  next.has(peerId) ? next.delete(peerId) : next.add(peerId);
  opened.value = next;
}

const members = computed<Member[]>(() => {
  const byPeer = new Map<string, FileTransfer[]>();

  for (const file of props.files) {
    const held = byPeer.get(file.peer_id);
    held ? held.push(file) : byPeer.set(file.peer_id, [file]);
  }

  const members: Member[] = [];

  for (const [peerId, files] of byPeer) {
    const contact = props.contacts.find((candidate) => candidate.peer_id === peerId);

    files.sort((a, b) => b.sent_at - a.sent_at);

    members.push({
      peerId,
      // Somebody can be in a group without being a contact of ours, in which
      // case their id is all we have to call them.
      name: contact?.nickname ?? shortPeerId(peerId),
      files,
      done: files.filter((file) => file.status === "complete").length,
      total: files.length,
      failed: files.filter((file) => file.status === "failed"),
      active:
        files.find(
          (file) => file.status === "transferring" || file.status === "pending",
        ) ?? null,
      bytes: files.reduce((total, file) => total + file.size, 0),
      sent: files.reduce((total, file) => total + file.transferred, 0),
    });
  }

  // Anything wanting attention first, then anything still moving, then the rest.
  return members.sort((a, b) => {
    const weight = (m: Member) => (m.failed.length ? 0 : m.active ? 1 : 2);
    return weight(a) - weight(b) || a.name.localeCompare(b.name);
  });
});

/** The one line a closed member has to say everything in. */
function summarise(member: Member): string {
  if (member.failed.length > 0) {
    const which = member.total > 1 ? ` of ${member.total}` : "";
    return `${member.failed.length}${which} did not go`;
  }

  if (member.active) {
    return `${member.done + 1} of ${member.total}, ${member.active.name}`;
  }

  return `${member.done} of ${member.total} · ${describeSize(member.bytes)}`;
}

/** How far along everything for this member is, taken together. */
function progress(member: Member): number {
  if (member.bytes === 0) {
    return 100;
  }

  return Math.min(100, Math.round((member.sent / member.bytes) * 100));
}
</script>

<template>
  <section class="group">
    <header class="group-header">
      <span class="who">
        <span class="name">{{ group.name }}</span>
        <span class="summary">
          {{ members.length }}
          {{ members.length === 1 ? "person" : "people" }} · {{ files.length }}
          {{ files.length === 1 ? "transfer" : "transfers" }}
        </span>
      </span>

      <button
        class="group-add"
        :title="`Choose files to send to everybody in ${group.name}`"
        @click="emit('add', group.id)"
      >
        Add files
      </button>
    </header>

    <ul class="members">
      <li v-for="member in members" :key="member.peerId" class="member">
        <div class="member-row" :class="{ trouble: member.failed.length > 0 }">
          <button
            class="disclose"
            :aria-expanded="opened.has(member.peerId)"
            :title="opened.has(member.peerId) ? 'Hide the files' : 'Show the files'"
            @click="toggle(member.peerId)"
          >
            <span class="chevron" :class="{ open: opened.has(member.peerId) }">▸</span>
          </button>

          <span
            class="presence"
            :class="{ online: onlinePeers.has(member.peerId) }"
            :title="onlinePeers.has(member.peerId) ? 'Online' : 'Offline'"
          />

          <span class="member-text">
            <span class="member-name">{{ member.name }}</span>
            <span class="member-status">{{ summarise(member) }}</span>

            <span v-if="member.active" class="progress">
              <span class="bar" :style="{ width: `${progress(member)}%` }" />
            </span>
          </span>

          <!-- Everything that failed for this one person, in one press. The
               others are not disturbed. -->
          <button
            v-if="member.failed.length > 0"
            class="resume"
            :title="`Try again for ${member.name}`"
            @click="emit('resumeMember', member.failed)"
          >
            Resume
          </button>
        </div>

        <ul v-if="opened.has(member.peerId)" class="files">
          <FileRow
            v-for="file in member.files"
            :key="file.id"
            :file="file"
            :newly-arrived="newlyArrived"
            @open="emit('open', $event)"
            @reveal="emit('reveal', $event)"
            @resume="emit('resume', $event)"
          />
        </ul>
      </li>
    </ul>
  </section>
</template>

<style scoped>
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

/* Members ---------------------------------------------------------------- */

.members {
  margin: 0;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background-color: var(--bg);
  list-style: none;
  overflow: hidden;
}

.member + .member {
  border-top: 1px solid var(--border);
}

.member-row {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 9px 12px;
}

/* Somebody who needs looking at. The point of the summary is that you can tell
   without opening anything. */
.member-row.trouble .member-status {
  color: var(--danger);
}

.disclose {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 20px;
  height: 20px;
  border-radius: var(--radius-sm);
  color: var(--text-faint);
}

.disclose:hover {
  background-color: var(--bg-hover);
  color: var(--text);
}

.chevron {
  font-size: 10px;
  transition: transform 0.15s ease;
}

.chevron.open {
  transform: rotate(90deg);
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

.member-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-width: 0;
}

.member-name {
  overflow: hidden;
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.member-status {
  font-size: 11px;
  color: var(--text-faint);
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

.resume {
  flex: none;
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

/* The files behind one member, indented so the nesting reads. */
.files {
  margin: 0;
  padding: 0 0 0 29px;
  border-top: 1px solid var(--border);
  background-color: var(--bg-sunken);
  list-style: none;
}
</style>
