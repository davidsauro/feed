<script setup lang="ts">
/**
 * The list of saved contacts in the sidebar.
 *
 * Owns nothing but which row is being renamed. Selecting a contact and saving a
 * new nickname are both reported upward, because they need the database.
 */
import { nextTick, ref } from "vue";
import type { Contact } from "../types";
import { shortPeerId } from "../types";

const props = defineProps<{
  contacts: Contact[];
  /** Peer IDs mDNS can currently see. */
  onlinePeers: Set<string>;
  /** Peer IDs with at least one message we haven't shown yet. */
  unread: Record<string, boolean>;
  selectedPeerId: string | null;
}>();

const emit = defineEmits<{
  select: [contact: Contact];
  rename: [peerId: string, nickname: string];
  /** Asks to remove the contact. App.vue confirms before anything is deleted. */
  remove: [contact: Contact];
}>();

const renamingPeerId = ref<string | null>(null);
const draftNickname = ref("");
const renameInput = ref<HTMLInputElement | null>(null);

async function startRename(contact: Contact) {
  renamingPeerId.value = contact.peer_id;
  draftNickname.value = contact.nickname;

  await nextTick();
  renameInput.value?.select();
}

function cancelRename() {
  renamingPeerId.value = null;
}

function commitRename(peerId: string) {
  const nickname = draftNickname.value.trim();
  if (!nickname) {
    return;
  }

  emit("rename", peerId, nickname);
  renamingPeerId.value = null;
}

function isOnline(peerId: string): boolean {
  return props.onlinePeers.has(peerId);
}
</script>

<template>
  <section class="contacts">
    <h2 class="section-title">
      Contacts
      <span class="count">{{ contacts.length }}</span>
    </h2>

    <p v-if="contacts.length === 0" class="empty">
      No contacts yet. Add a discovered peer below to start a conversation.
    </p>

    <ul v-else class="list">
      <li v-for="contact in contacts" :key="contact.peer_id">
        <!-- Rename mode: the row becomes a small form. -->
        <div v-if="renamingPeerId === contact.peer_id" class="rename">
          <input
            :ref="(el) => (renameInput = el as HTMLInputElement | null)"
            v-model="draftNickname"
            type="text"
            @keyup.enter="commitRename(contact.peer_id)"
            @keyup.escape="cancelRename"
          />
          <button
            class="icon-button confirm"
            title="Save"
            :disabled="!draftNickname.trim()"
            @click="commitRename(contact.peer_id)"
          >
            ✓
          </button>
          <button class="icon-button" title="Cancel" @click="cancelRename">
            ✕
          </button>
        </div>

        <!-- Normal mode. A div rather than a button, because it holds the
             rename button and nesting buttons is invalid. -->
        <div
          v-else
          class="row"
          role="button"
          tabindex="0"
          :class="{ selected: selectedPeerId === contact.peer_id }"
          :title="contact.peer_id"
          @click="emit('select', contact)"
          @keyup.enter="emit('select', contact)"
        >
          <span
            class="status-dot"
            :class="{ online: isOnline(contact.peer_id) }"
            :title="isOnline(contact.peer_id) ? 'Online' : 'Offline'"
          />

          <span class="text">
            <span class="nickname">{{ contact.nickname }}</span>
            <span class="peer-id">{{ shortPeerId(contact.peer_id) }}</span>
          </span>

          <span v-if="unread[contact.peer_id]" class="unread" title="New message" />

          <span class="row-actions">
            <button
              class="icon-button"
              title="Rename"
              @click.stop="startRename(contact)"
            >
              ✎
            </button>

            <button
              class="icon-button remove"
              title="Remove contact"
              @click.stop="emit('remove', contact)"
            >
              <svg
                viewBox="0 0 24 24"
                width="13"
                height="13"
                stroke="currentColor"
                stroke-width="2"
                fill="none"
                stroke-linecap="round"
              >
                <polyline points="3 6 5 6 21 6" />
                <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
                <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
              </svg>
            </button>
          </span>
        </div>
      </li>
    </ul>
  </section>
</template>

<style scoped>
.contacts {
  display: flex;
  flex-direction: column;
  min-height: 0;
  padding: 12px 8px;
}

.section-title {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0 0 6px;
  padding: 0 6px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--text-faint);
}

.count {
  font-size: 10px;
  letter-spacing: 0;
  color: var(--text-faint);
}

.empty {
  margin: 4px 6px;
  font-size: 12px;
  color: var(--text-faint);
}

.list {
  margin: 0;
  padding: 0;
  overflow-y: auto;
  list-style: none;
}

.row {
  display: flex;
  align-items: center;
  gap: 9px;
  width: 100%;
  padding: 7px 8px;
  border-radius: var(--radius-sm);
  text-align: left;
}

.row:hover {
  background-color: var(--bg-hover);
}

.row.selected {
  background-color: var(--bg-active);
}

.status-dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background-color: var(--offline);
}

.status-dot.online {
  background-color: var(--online);
}

.text {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
}

.nickname {
  overflow: hidden;
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.peer-id {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--text-faint);
}

.unread {
  flex: none;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background-color: var(--accent);
}

.icon-button {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--text-faint);
  cursor: pointer;
}

.icon-button:hover:not(:disabled) {
  background-color: var(--bg-hover);
  color: var(--text);
}

.confirm:not(:disabled) {
  color: var(--online);
}

/* Row actions stay hidden until the row is hovered, so the resting state of the
   list is just names. Focus reveals them too, for keyboard use. */
.row-actions {
  display: flex;
  gap: 2px;
  flex: none;
  opacity: 0;
}

.row:hover .row-actions,
.row-actions:focus-within {
  opacity: 1;
}

.remove:hover:not(:disabled) {
  background-color: var(--danger-bg);
  color: var(--danger);
}

.rename {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 4px 8px;
}
</style>
