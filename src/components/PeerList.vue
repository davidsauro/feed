<script setup lang="ts">
/**
 * Finding people: peers mDNS has turned up, and a way to add somebody by hand.
 *
 * Adding by hand exists because discovery only reaches the local network.
 * Somebody on the other side of the internet, reachable through a relay server,
 * never appears in this list and has to be added from their peer id, which they
 * copy from the top of their own sidebar and send you however they like.
 *
 * Each discovered row keeps its own nickname draft, keyed by peer ID. A single
 * shared input would put whatever you typed into every row at once.
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

/** The add-by-id form, which stays out of the way until it is wanted. */
const addingById = ref(false);
const typedPeerId = ref("");
const typedNickname = ref("");

const canAddTyped = () =>
  typedPeerId.value.trim().length > 0 && typedNickname.value.trim().length > 0;

function addTyped() {
  if (!canAddTyped()) {
    return;
  }

  // Whether this is a real peer id is the backend's to say. Guessing at the
  // format here would mean two places to keep in step.
  emit("add", typedPeerId.value.trim(), typedNickname.value.trim());

  typedPeerId.value = "";
  typedNickname.value = "";
  addingById.value = false;
}

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

      <!-- The only way to reach somebody who is not on this network, since
           discovery does not leave it. -->
      <button v-if="!addingById" class="by-id-toggle" @click="addingById = true">
        + Add someone by their ID
      </button>

      <div v-else class="by-id">
        <input
          v-model="typedPeerId"
          class="id-input"
          type="text"
          placeholder="Their peer ID (12D3KooW…)"
          spellcheck="false"
          autofocus
        />
        <input
          v-model="typedNickname"
          type="text"
          placeholder="What to call them"
          @keyup.enter="addTyped"
        />

        <div class="by-id-actions">
          <button class="cancel" @click="addingById = false">Cancel</button>
          <button class="add-button" :disabled="!canAddTyped()" @click="addTyped">
            Add
          </button>
        </div>

        <p class="by-id-hint">
          They send you the ID from the top of their own sidebar, where clicking
          it copies it. They have to add you the same way before either of you
          can send anything.
        </p>
      </div>
    </div>
  </section>
</template>

<style scoped>
/* Add by ID ------------------------------------------------------------- */

.by-id-toggle {
  width: 100%;
  margin-top: 6px;
  padding: 7px;
  border: 1px dashed var(--border-strong);
  border-radius: var(--radius-sm);
  font-size: 12px;
  color: var(--text-muted);
}

.by-id-toggle:hover {
  background-color: var(--bg-hover);
  color: var(--text);
}

.by-id {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 6px;
  padding: 9px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
}

.by-id input {
  width: 100%;
  padding: 6px 8px;
  font-size: 12px;
}

.id-input {
  font-family: var(--font-mono);
  font-size: 11px;
}

.by-id-actions {
  display: flex;
  gap: 6px;
  justify-content: flex-end;
}

.cancel {
  padding: 5px 9px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  color: var(--text-faint);
}

.cancel:hover {
  color: var(--text);
}

.by-id-hint {
  margin: 2px 0 0;
  font-size: 11px;
  line-height: 1.4;
  color: var(--text-faint);
}

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
