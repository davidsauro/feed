<script setup lang="ts">
/**
 * The list of group conversations in the sidebar.
 *
 * Groups have no presence indicator: a group is reachable if any member is, and
 * that isn't something worth reducing to one dot.
 */
import type { Group } from "../types";

defineProps<{
  groups: Group[];
  /** Group id to whether it has messages we haven't shown yet. */
  unread: Record<string, boolean>;
  selectedGroupId: string | null;
  /** A group needs someone to talk to, so this is false with no contacts. */
  canCreate: boolean;
}>();

const emit = defineEmits<{
  select: [group: Group];
  /** Asks to leave. App.vue confirms before anything is deleted. */
  leave: [group: Group];
  create: [];
}>();
</script>

<template>
  <section class="groups">
    <h2 class="section-title">
      Groups
      <span class="count">{{ groups.length }}</span>

      <button
        class="new-button"
        title="New group"
        :disabled="!canCreate"
        @click="emit('create')"
      >
        +
      </button>
    </h2>

    <p v-if="groups.length === 0" class="empty">
      {{
        canCreate
          ? "No groups yet."
          : "Add a contact before starting a group."
      }}
    </p>

    <ul v-else class="list">
      <li v-for="group in groups" :key="group.id">
        <div
          class="row"
          role="button"
          tabindex="0"
          :class="{ selected: selectedGroupId === group.id }"
          @click="emit('select', group)"
          @keyup.enter="emit('select', group)"
        >
          <svg
            class="icon"
            viewBox="0 0 24 24"
            width="14"
            height="14"
            stroke="currentColor"
            stroke-width="2"
            fill="none"
          >
            <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
            <circle cx="9" cy="7" r="4" />
            <path d="M23 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75" />
          </svg>

          <span class="text">
            <span class="name">{{ group.name }}</span>
            <span class="members">
              {{ group.members.length }}
              {{ group.members.length === 1 ? "member" : "members" }}
            </span>
          </span>

          <span v-if="unread[group.id]" class="unread" title="New message" />

          <button
            class="icon-button leave"
            title="Leave group"
            @click.stop="emit('leave', group)"
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
              <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
              <polyline points="16 17 21 12 16 7" />
              <line x1="21" y1="12" x2="9" y2="12" />
            </svg>
          </button>
        </div>
      </li>
    </ul>
  </section>
</template>

<style scoped>
.groups {
  display: flex;
  flex-direction: column;
  flex: none;
  min-height: 0;
  padding: 8px;
  border-top: 1px solid var(--border);
}

.section-title {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0 0 4px;
  padding: 0 6px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--text-faint);
}

.count {
  letter-spacing: 0;
}

.new-button {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  margin-left: auto;
  border-radius: var(--radius-sm);
  font-size: 15px;
  line-height: 1;
  color: var(--text-faint);
}

.new-button:hover:not(:disabled) {
  background-color: var(--bg-hover);
  color: var(--accent);
}

.empty {
  margin: 4px 6px;
  font-size: 12px;
  color: var(--text-faint);
}

.list {
  margin: 0;
  padding: 0;
  /* Keeps a long group list from squeezing out the contacts above it. */
  max-height: 30vh;
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

.icon {
  flex: none;
  color: var(--text-faint);
}

.text {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
}

.name {
  overflow: hidden;
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.members {
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
  opacity: 0;
}

.row:hover .icon-button,
.icon-button:focus-visible {
  opacity: 1;
}

.leave:hover {
  background-color: var(--danger-bg);
  color: var(--danger);
}
</style>
