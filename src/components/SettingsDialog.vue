<script setup lang="ts">
/**
 * Settings.
 *
 * Turning encryption on takes a passphrase, and turning it off gives up the
 * protection, so both are confirmed here rather than happening the instant the
 * switch moves. Neither is instant either — the whole database is rewritten —
 * so both report progress.
 */
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import ThemeToggle from "./ThemeToggle.vue";
import type { Server, ServerStatus } from "../types";
import { describeDuration, describeServer } from "../types";

const props = defineProps<{
  encryptionEnabled: boolean;
  /** The name this node asks others to call it. */
  displayName: string;
  /** Relay servers this node is configured to use. */
  servers: Server[];
  /**
   * Measurements for each server: round trip, uptime, and the last failure.
   *
   * Deliberately not the source of whether a server is up. That comes from
   * `onlinePeers`, which is live. If this held it too, a stale reading and a live
   * dot could contradict each other on the same row.
   */
  serverStatus: ServerStatus[];
  /**
   * Peers reachable right now. A server is an ordinary peer once connected, so
   * this is what says whether each one is actually working.
   */
  onlinePeers: Set<string>;
  testingServers: boolean;
}>();

const emit = defineEmits<{
  enable: [passphrase: string];
  disable: [];
  rename: [name: string];
  addServer: [address: string];
  removeServer: [address: string];
  testServers: [];
  refreshServers: [];
  close: [];
}>();

/**
 * Ticks so an uptime that is on screen keeps counting.
 *
 * Only runs while this dialog is open, which is the only time anything reads it.
 */
const now = ref(Date.now());
let clock: ReturnType<typeof setInterval> | null = null;

const isOnline = (server: Server) => props.onlinePeers.has(server.peer_id);

const statusOf = (server: Server) =>
  props.serverStatus.find((status) => status.address === server.address) ?? null;

/**
 * The measurements line for one server.
 *
 * Reads as one short phrase rather than a row of labelled fields, because there
 * are only ever two or three numbers and a sentence is easier to take in.
 */
function describeMetrics(server: Server): string {
  const status = statusOf(server);

  if (!isOnline(server)) {
    return status?.last_error ?? "not connected";
  }

  const parts = [];

  if (status?.round_trip_ms !== null && status?.round_trip_ms !== undefined) {
    parts.push(`${status.round_trip_ms} ms`);
  }

  if (status?.connected_at) {
    parts.push(`up ${describeDuration(now.value - status.connected_at)}`);
  }

  // A server that is connected but has told us nothing else yet.
  return parts.length > 0 ? parts.join(" · ") : "connected";
}

const serverDraft = ref("");

function addServer() {
  const address = serverDraft.value.trim();
  if (!address) {
    return;
  }

  emit("addServer", address);

  // Cleared straight away rather than on success, because the field coming back
  // empty is what says the address was taken. A rejected one is reported
  // separately.
  serverDraft.value = "";
}

/** Longest name the backend will advertise; kept in step with MAX_DISPLAY_NAME. */
const NAME_LIMIT = 32;

const nameDraft = ref(props.displayName);

function saveName() {
  const name = nameDraft.value.trim();

  if (name !== props.displayName.trim()) {
    emit("rename", name);
  }
}

/** Which confirmation is on screen, if any. */
const pending = ref<"enable" | "disable" | null>(null);
const passphrase = ref("");
const confirmation = ref("");
const passphraseInput = ref<HTMLInputElement | null>(null);

/** Set while the database is being rewritten, which is not instant. */
const busy = ref(false);

const mismatch = () =>
  confirmation.value.length > 0 && passphrase.value !== confirmation.value;

const canEnable = () =>
  passphrase.value.length > 0 && passphrase.value === confirmation.value;

async function onToggle(event: Event) {
  // The switch reflects what's saved, not what's been asked for. Put it back and
  // let the confirmation below decide.
  const input = event.target as HTMLInputElement;
  input.checked = props.encryptionEnabled;

  pending.value = props.encryptionEnabled ? "disable" : "enable";
  passphrase.value = "";
  confirmation.value = "";

  await nextTick();
  passphraseInput.value?.focus();
}

function cancel() {
  pending.value = null;
  passphrase.value = "";
  confirmation.value = "";
}

function confirmEnable() {
  if (!canEnable()) {
    return;
  }

  busy.value = true;
  emit("enable", passphrase.value);
}

function confirmDisable() {
  busy.value = true;
  emit("disable");
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape" && !busy.value) {
    emit("close");
  }
}

onMounted(() => {
  window.addEventListener("keydown", onKeydown);

  // What is on screen should be current when it appears, not whatever was last
  // measured minutes ago.
  emit("refreshServers");

  clock = setInterval(() => {
    now.value = Date.now();
  }, 1000);
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown);

  if (clock) {
    clearInterval(clock);
  }
});
</script>

<template>
  <Teleport to="body">
    <div class="backdrop" @click.self="busy || emit('close')">
      <div class="dialog" role="dialog" aria-modal="true">
        <header class="header">
          <h2 class="title">Settings</h2>
          <button class="close" title="Close" :disabled="busy" @click="emit('close')">
            ✕
          </button>
        </header>

        <section class="section">
          <h3 class="section-title">This node</h3>

          <div class="row">
            <div class="row-text">
              <span class="label">Your name</span>
              <span class="hint">
                Shown to other nodes that discover you. They can still call you
                something else once they add you.
              </span>
            </div>

            <input
              v-model="nameDraft"
              class="name-input"
              type="text"
              placeholder="Unnamed"
              :maxlength="NAME_LIMIT"
              @blur="saveName"
              @keyup.enter="saveName"
            />
          </div>
        </section>

        <section class="section">
          <h3 class="section-title">Appearance</h3>
          <div class="row">
            <div class="row-text">
              <span class="label">Theme</span>
              <span class="hint">Mirror system, light or dark</span>
            </div>
            <ThemeToggle />
          </div>
        </section>

        <section class="section">
          <h3 class="section-title">Servers</h3>

          <p class="hint section-hint">
            A server relays messages between people who are not on the same
            network. It carries traffic without being able to read it, so adding
            one is a decision about reach rather than about trust. On a local
            network you do not need one at all.
          </p>

          <ul v-if="servers.length" class="servers">
            <li
              v-for="server in servers"
              :key="server.address"
              class="server"
              :class="{ offline: !isOnline(server) }"
            >
              <span class="server-dot" :class="{ online: isOnline(server) }" />

              <span class="server-text">
                <span class="server-line">
                  <span class="server-host">{{ describeServer(server) }}</span>
                  <span class="server-state">
                    {{ isOnline(server) ? "Online" : "Offline" }}
                  </span>
                </span>

                <!-- Round trip, uptime, or why it could not be reached. -->
                <span class="server-metrics">{{ describeMetrics(server) }}</span>

                <!-- Which server this is, kept visible: it is what stops
                     something else answering at that address from passing
                     itself off as this one. -->
                <code class="server-id">{{ server.peer_id }}</code>
              </span>

              <button
                class="server-remove"
                title="Remove this server"
                @click="emit('removeServer', server.address)"
              >
                ✕
              </button>
            </li>
          </ul>

          <div v-if="servers.length" class="server-actions">
            <button
              class="secondary"
              :disabled="testingServers"
              @click="emit('testServers')"
            >
              {{ testingServers ? "Testing…" : "Test connections" }}
            </button>

            <span class="hint test-hint">
              Dials anything not connected and reports what came back. Takes a
              few seconds.
            </span>
          </div>

          <div class="add-server">
            <input
              v-model="serverDraft"
              class="server-input"
              type="text"
              placeholder="/ip4/203.0.113.7/tcp/4001/p2p/12D3KooW…"
              spellcheck="false"
              @keyup.enter="addServer"
            />
            <button class="primary" :disabled="!serverDraft.trim()" @click="addServer">
              Add
            </button>
          </div>

          <p class="hint">
            The address a server prints when it starts, including the
            <code>/p2p/</code> part. Without that this node would connect to
            whatever answers at that address rather than to the server you meant.
          </p>
        </section>

        <section class="section">
          <h3 class="section-title">Security</h3>

          <div class="row">
            <div class="row-text">
              <span class="label">Encryption at rest</span>
              <span class="hint">This will encrypt your on-device data storage.</span>
            </div>

            <label class="switch" :class="{ disabled: busy || pending !== null }">
              <input
                type="checkbox"
                :checked="encryptionEnabled"
                :disabled="busy || pending !== null"
                @click="onToggle"
              />
              <span class="track"><span class="thumb" /></span>
            </label>
          </div>

          <!-- Turning it on. -->
          <div v-if="pending === 'enable'" class="panel">
            <p class="warning">
              <svg
                viewBox="0 0 24 24"
                width="15"
                height="15"
                stroke="currentColor"
                stroke-width="2"
                fill="none"
                stroke-linecap="round"
              >
                <path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
                <line x1="12" y1="9" x2="12" y2="13" />
                <line x1="12" y1="17" x2="12.01" y2="17" />
              </svg>
              <span>
                If you turn this option on, you must be sure to remember your
                passphrase. If you forget it, you will lose your data. It is not
                stored anywhere and cannot be recovered or reset.
              </span>
            </p>

            <label class="field">
              <span class="field-label">Passphrase</span>
              <input
                ref="passphraseInput"
                v-model="passphrase"
                type="password"
                :disabled="busy"
                autocomplete="new-password"
              />
            </label>

            <label class="field">
              <span class="field-label">Confirm passphrase</span>
              <input
                v-model="confirmation"
                type="password"
                :disabled="busy"
                autocomplete="new-password"
                @keyup.enter="confirmEnable"
              />
            </label>

            <p v-if="mismatch()" class="mismatch">Those don't match.</p>

            <div class="actions">
              <button class="secondary" :disabled="busy" @click="cancel">Cancel</button>
              <button class="primary" :disabled="busy || !canEnable()" @click="confirmEnable">
                {{ busy ? "Encrypting…" : "Encrypt my data" }}
              </button>
            </div>
          </div>

          <!-- Turning it off. -->
          <div v-else-if="pending === 'disable'" class="panel">
            <p class="hint">
              Your contacts, groups, and chat history will be written back to
              disk unencrypted, readable by anything with access to this
              computer. No passphrase will be asked for at startup.
            </p>

            <div class="actions">
              <button class="secondary" :disabled="busy" @click="cancel">Cancel</button>
              <button class="danger" :disabled="busy" @click="confirmDisable">
                {{ busy ? "Decrypting…" : "Turn off encryption" }}
              </button>
            </div>
          </div>

          <p v-else-if="encryptionEnabled" class="status">
            <span class="status-dot" />
            Your data is encrypted. A passphrase is required at startup.
          </p>
        </section>
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
  gap: 18px;
  width: 100%;
  max-width: 440px;
  max-height: 100%;
  overflow-y: auto;
  padding: 18px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius);
  background-color: var(--bg);
  box-shadow: 0 12px 32px rgba(8, 11, 15, 0.28);
}

.header {
  display: flex;
  align-items: center;
}

.title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
}

.close {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  margin-left: auto;
  border-radius: var(--radius-sm);
  color: var(--text-faint);
}

.close:hover:not(:disabled) {
  background-color: var(--bg-hover);
  color: var(--text);
}

.section {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.section-title {
  margin: 0;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--text-faint);
}

.row {
  display: flex;
  align-items: center;
  gap: 16px;
}

.row-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex: 1;
  min-width: 0;
}

.label {
  font-weight: 500;
}

.hint {
  font-size: 12px;
  color: var(--text-muted);
}

.name-input {
  flex: none;
  width: 150px;
}

/* Switch ---------------------------------------------------------------- */

.switch {
  flex: none;
  cursor: pointer;
}

.switch.disabled {
  cursor: default;
  opacity: 0.6;
}

.switch input {
  position: absolute;
  opacity: 0;
  width: 0;
  height: 0;
}

.track {
  display: flex;
  align-items: center;
  width: 36px;
  height: 20px;
  padding: 2px;
  border-radius: var(--radius-pill);
  background-color: var(--border-strong);
  transition: background-color 0.15s ease;
}

.thumb {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background-color: var(--bg);
  transition: transform 0.15s ease;
}

.switch input:checked + .track {
  background-color: var(--accent);
}

.switch input:checked + .track .thumb {
  transform: translateX(16px);
}

.switch input:focus-visible + .track {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

/* Confirmation panels --------------------------------------------------- */

.panel {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background-color: var(--bg-sunken);
}

.warning {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin: 0;
  padding: 9px 11px;
  border: 1px solid var(--danger);
  border-radius: var(--radius-sm);
  background-color: var(--danger-bg);
  color: var(--danger);
  font-size: 13px;
}

.warning svg {
  flex: none;
  margin-top: 2px;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.field-label {
  font-size: 12px;
  color: var(--text-muted);
}

.mismatch {
  margin: 0;
  font-size: 12px;
  color: var(--danger);
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.primary,
.secondary,
.danger {
  padding: 7px 14px;
  border-radius: var(--radius-sm);
  font-weight: 500;
}

.secondary {
  border-color: var(--border-strong);
  color: var(--text);
}

.secondary:hover:not(:disabled) {
  background-color: var(--bg-hover);
}

.primary {
  background-color: var(--accent);
  color: var(--accent-contrast);
}

.primary:hover:not(:disabled) {
  background-color: var(--accent-hover);
}

.danger {
  background-color: var(--danger-solid);
  color: var(--danger-solid-text);
}

.danger:hover:not(:disabled) {
  filter: brightness(1.08);
}

.status {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.status-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background-color: var(--online);
}

/* Servers ---------------------------------------------------------------- */

.section-hint {
  margin: 0 0 10px;
}

.servers {
  margin: 0 0 10px;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  list-style: none;
  overflow: hidden;
}

.server {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 9px 11px;
}

.server + .server {
  border-top: 1px solid var(--border);
}

/* Whether this server is actually reachable, which is the one thing a list of
   configured servers cannot tell you on its own. */
.server-dot {
  flex: none;
  width: 8px;
  height: 8px;
  margin-top: 5px;
  border-radius: 50%;
  background-color: var(--offline);
}

.server-dot.online {
  background-color: var(--online);
}

.server-text {
  display: flex;
  flex-direction: column;
  gap: 1px;
  flex: 1;
  min-width: 0;
}

.server-line {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.server-host {
  overflow: hidden;
  flex: 1;
  font-family: var(--font-mono);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Said in words as well as by the dot. A colour alone is not something
   everybody can read. */
.server-state {
  flex: none;
  font-size: 11px;
  font-weight: 600;
  color: var(--online);
}

.server.offline .server-state {
  color: var(--text-faint);
}

.server-metrics {
  font-size: 11px;
  color: var(--text-muted);
}

/* The reason a server could not be reached, which is the useful thing on the
   screen when one cannot. */
.server.offline .server-metrics {
  color: var(--danger);
}

.server-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
}

.test-hint {
  flex: 1;
  min-width: 0;
}

.server-id {
  overflow: hidden;
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--text-faint);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.server-remove {
  flex: none;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  color: var(--text-faint);
}

.server-remove:hover {
  background-color: var(--bg-hover);
  color: var(--danger);
}

.add-server {
  display: flex;
  gap: 8px;
  margin-bottom: 8px;
}

.server-input {
  flex: 1;
  min-width: 0;
  padding: 8px 10px;
  font-family: var(--font-mono);
  font-size: 12px;
}
</style>
