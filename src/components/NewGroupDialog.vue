<script setup lang="ts">
/**
 * Dialog for starting a group: a name and who's in it.
 *
 * Only contacts can be picked. Group messages are dropped on arrival unless
 * they're from a contact, so inviting a stranger would produce a group whose
 * messages you'd never see.
 */
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import type { Contact } from "../types";
import { shortPeerId } from "../types";

defineProps<{
  contacts: Contact[];
}>();

const emit = defineEmits<{
  create: [name: string, memberPeerIds: string[]];
  cancel: [];
}>();

const name = ref("");
const selected = ref<Set<string>>(new Set());
const nameInput = ref<HTMLInputElement | null>(null);

function toggle(peerId: string) {
  if (selected.value.has(peerId)) {
    selected.value.delete(peerId);
  } else {
    selected.value.add(peerId);
  }
}

function create() {
  const trimmed = name.value.trim();
  if (!trimmed || selected.value.size === 0) {
    return;
  }

  emit("create", trimmed, Array.from(selected.value));
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    emit("cancel");
  }
}

onMounted(async () => {
  window.addEventListener("keydown", onKeydown);
  await nextTick();
  nameInput.value?.focus();
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown);
});
</script>

<template>
  <Teleport to="body">
    <div class="backdrop" @click.self="emit('cancel')">
      <div class="dialog" role="dialog" aria-modal="true">
        <h2 class="title">New group</h2>

        <label class="field">
          <span class="label">Name</span>
          <input
            ref="nameInput"
            v-model="name"
            type="text"
            placeholder="Weekend plans"
            @keyup.enter="create"
          />
        </label>

        <div class="field">
          <span class="label">
            Members
            <span v-if="selected.size" class="chosen">{{ selected.size }} chosen</span>
          </span>

          <ul class="members">
            <li v-for="contact in contacts" :key="contact.peer_id">
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
        </div>

        <p class="hint">
          Everyone you pick is invited straight away, and will see the group
          appear in their sidebar.
        </p>

        <div class="actions">
          <button class="cancel" @click="emit('cancel')">Cancel</button>
          <button
            class="confirm"
            :disabled="!name.trim() || selected.size === 0"
            @click="create"
          >
            Create group
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
  gap: 14px;
  width: 100%;
  max-width: 400px;
  max-height: 100%;
  padding: 18px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius);
  background-color: var(--bg);
  box-shadow: 0 12px 32px rgba(8, 11, 15, 0.28);
}

.title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-height: 0;
}

.label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--text-faint);
}

.chosen {
  margin-left: auto;
  letter-spacing: 0;
  text-transform: none;
  color: var(--accent);
}

.members {
  margin: 0;
  padding: 4px;
  max-height: 240px;
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

.hint {
  margin: 0;
  font-size: 12px;
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
