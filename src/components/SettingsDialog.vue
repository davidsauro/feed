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

const props = defineProps<{
  encryptionEnabled: boolean;
}>();

const emit = defineEmits<{
  enable: [passphrase: string];
  disable: [];
  close: [];
}>();

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

onMounted(() => window.addEventListener("keydown", onKeydown));
onUnmounted(() => window.removeEventListener("keydown", onKeydown));
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
</style>
