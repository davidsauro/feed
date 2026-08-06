<script setup lang="ts">
/**
 * Peers mDNS has found on the local network that aren't contacts yet.
 *
 * Each row keeps its own nickname draft, keyed by peer ID. A single shared input
 * would put whatever you typed into every row at once.
 */
import { ref } from "vue";
import { shortPeerId } from "../types";

const props = defineProps<{
  peers: string[];
  /**
   * What each node calls itself, by peer id.
   *
   * A claim rather than an identity — anyone can call themselves anything — so
   * the peer id stays on screen beside it, and this only ever fills in the
   * nickname box as a starting point.
   */
  names: Record<string, string>;
}>();

const emit = defineEmits<{
  add: [peerId: string, nickname: string];
}>();

const expanded = ref(true);

/** Edits in progress. A peer with no entry hasn't been typed over yet. */
const drafts = ref<Record<string, string>>({});

/** What's in the box: what the user typed, or the name the node offered. */
function draftFor(peerId: string): string {
  return drafts.value[peerId] ?? props.names[peerId] ?? "";
}

function setDraft(peerId: string, value: string) {
  drafts.value[peerId] = value;
}

function add(peerId: string) {
  const nickname = draftFor(peerId).trim();
  if (!nickname) {
    return;
  }

  emit("add", peerId, nickname);
  delete drafts.value[peerId];
}
</script>

<template>
  <section class="discovered">
    <button class="section-title" @click="expanded = !expanded">
      <span class="chevron" :class="{ expanded }">▸</span>
      Discovered
      <span class="count">{{ peers.length }}</span>
    </button>

    <div v-if="expanded" class="body">
      <p v-if="peers.length === 0" class="empty">
        Nothing new on this network.
      </p>

      <ul v-else class="list">
        <li v-for="peer in peers" :key="peer" class="row">
          <span v-if="names[peer]" class="claimed-name">{{ names[peer] }}</span>
          <code class="peer-id" :title="peer">{{ shortPeerId(peer) }}</code>

          <div class="add-row">
            <input
              type="text"
              placeholder="Name this peer…"
              :value="draftFor(peer)"
              @input="setDraft(peer, ($event.target as HTMLInputElement).value)"
              @keyup.enter="add(peer)"
            />
            <button
              class="add-button"
              :disabled="!draftFor(peer).trim()"
              title="Add as contact"
              @click="add(peer)"
            >
              Add
            </button>
          </div>
        </li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.discovered {
  display: flex;
  flex-direction: column;
  min-height: 0;
  padding: 8px;
  border-top: 1px solid var(--border);
}

.section-title {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 4px 6px;
  border-radius: var(--radius-sm);
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--text-faint);
}

.section-title:hover {
  background-color: var(--bg-hover);
  color: var(--text-muted);
}

.chevron {
  display: inline-block;
  font-size: 9px;
  transition: transform 0.15s ease;
}

.chevron.expanded {
  transform: rotate(90deg);
}

.count {
  margin-left: auto;
  letter-spacing: 0;
}

.body {
  min-height: 0;
  /* Keeps a busy network from pushing the contact list off screen. */
  max-height: 40vh;
  overflow-y: auto;
}

.empty {
  margin: 4px 6px 6px;
  font-size: 12px;
  color: var(--text-faint);
}

.list {
  margin: 0;
  padding: 0;
  list-style: none;
}

.row {
  padding: 8px 6px;
  border-radius: var(--radius-sm);
}

.row + .row {
  border-top: 1px solid var(--border);
}

.claimed-name {
  display: block;
  overflow: hidden;
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.peer-id {
  display: block;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-muted);
}

.add-row {
  display: flex;
  gap: 5px;
  margin-top: 6px;
}

.add-row input {
  flex: 1;
  padding: 5px 8px;
  font-size: 12px;
}

.add-button {
  flex: none;
  padding: 5px 10px;
  font-size: 12px;
  font-weight: 500;
  background-color: var(--accent);
  color: var(--accent-contrast);
}

.add-button:hover:not(:disabled) {
  background-color: var(--accent-hover);
}
</style>
