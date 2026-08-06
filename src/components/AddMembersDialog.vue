<script setup lang="ts">
/**
 * Adds contacts to a group that already exists.
 *
 * Also the way back in for someone who left, since leaving is theirs to undo
 * only by being asked again.
 *
 * Only contacts can be added, for the same reason they're the only ones who can
 * be in a new group: a message from someone we haven't added is dropped on
 * arrival, so inviting a stranger would make a member who never hears anything.
 */
import { onMounted, onUnmounted, ref } from "vue";
import type { Contact } from "../types";
import { shortPeerId } from "../types";

defineProps<{
  groupName: string;
  /** Contacts not already in the group. */
  candidates: Contact[];
}>();

const emit = defineEmits<{
  add: [peerIds: string[]];
  cancel: [];
}>();

const selected = ref<Set<string>>(new Set());

function toggle(peerId: string) {
  if (selected.value.has(peerId)) {
    selected.value.delete(peerId);
  } else {
    selected.value.add(peerId);
  }
}

function add() {
  if (selected.value.size > 0) {
    emit("add", Array.from(selected.value));
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    emit("cancel");
  }
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onUnmounted(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <Teleport to="body">
    <div class="backdrop" @click.self="emit('cancel')">
      <div class="dialog" role="dialog" aria-modal="true">
        <h2 class="title">Add to {{ groupName }}</h2>

        <p v-if="candidates.length === 0" class="empty">
          Everyone in your contacts is already in this group.
        </p>

        <ul v-else class="members">
          <li v-for="contact in candidates" :key="contact.peer_id">
            <label class="member">
              <input
                type="checkbox"
                :checked="selected.has(contact.peer_id)"
                @change="toggle(contact.peer_id)"
              />
              <span class="member-text">
                <span class="nickname">{{ contact.nickname }}</span>
                <code class="peer-id">{{ shortPeerId(contact.peer_id) }}</code>
              </span>
            </label>
          </li>
        </ul>

        <p v-if="candidates.length > 0" class="hint">
          Everyone in the group is told who the members are, so they can all
          reach each other.
        </p>

        <div class="actions">
          <button class="cancel" @click="emit('cancel')">Cancel</button>
          <button class="confirm" :disabled="selected.size === 0" @click="add">
            Add to group
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.backdrop {
  display: flex;
  align-items: center;
  justify-content: center;
  position: fixed;
  inset: 0;
  z-index: 10;
  padding: 24px;
  background-color: rgba(8, 11, 15, 0.5);
}

.dialog {
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: 100%;
  max-width: 380px;
  max-height: 100%;
  padding: 18px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius);
  background-color: var(--bg);
  box-shadow: 0 12px 32px rgba(8, 11, 15, 0.28);
}

.title {
  margin: 0;
  overflow: hidden;
  font-size: 15px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.empty,
.hint {
  margin: 0;
  font-size: 12px;
  color: var(--text-faint);
}

.members {
  margin: 0;
  padding: 4px;
  max-height: 260px;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  list-style: none;
}

.member {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 6px 7px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}

.member:hover {
  background-color: var(--bg-hover);
}

.member-text {
  display: flex;
  flex-direction: column;
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

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.cancel,
.confirm {
  padding: 7px 14px;
  border-radius: var(--radius-sm);
  font-weight: 500;
}

.cancel {
  border-color: var(--border-strong);
  color: var(--text);
}

.cancel:hover {
  background-color: var(--bg-hover);
}

.confirm {
  background-color: var(--accent);
  color: var(--accent-contrast);
}

.confirm:hover:not(:disabled) {
  background-color: var(--accent-hover);
}
</style>
