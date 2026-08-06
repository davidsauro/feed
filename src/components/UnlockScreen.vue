<script setup lang="ts">
/**
 * Shown at startup when the database is encrypted, in place of the whole app.
 *
 * Nothing loads behind it: with no passphrase the database cannot be read at
 * all, so there is nothing to show.
 *
 * The way out for a forgotten passphrase is deleting the data, which is
 * unrecoverable. It's kept behind a disclosure and asks the user to type the
 * word, because a button next to a passphrase box is far too easy to hit by
 * accident.
 */
import { nextTick, onMounted, ref } from "vue";

defineProps<{
  /** Message from the last failed attempt, if any. */
  error: string;
  /** Set while an attempt is in flight. */
  busy: boolean;
}>();

const emit = defineEmits<{
  unlock: [passphrase: string];
  reset: [];
}>();

const passphrase = ref("");
const passphraseInput = ref<HTMLInputElement | null>(null);

const showReset = ref(false);
const resetConfirmation = ref("");
const resetInput = ref<HTMLInputElement | null>(null);

/** Anything but the word itself leaves the button inert. */
const resetConfirmed = () => resetConfirmation.value.trim().toLowerCase() === "yes";

function unlock() {
  if (passphrase.value.length === 0) {
    return;
  }

  emit("unlock", passphrase.value);
}

async function revealReset() {
  showReset.value = true;
  await nextTick();
  resetInput.value?.focus();
}

onMounted(() => passphraseInput.value?.focus());
</script>

<template>
  <div class="screen">
    <div class="card">
      <svg
        class="lock"
        viewBox="0 0 24 24"
        width="28"
        height="28"
        stroke="currentColor"
        stroke-width="1.6"
        fill="none"
      >
        <rect x="3" y="11" width="18" height="11" rx="2" />
        <path d="M7 11V7a5 5 0 0 1 10 0v4" />
      </svg>

      <h1 class="title">This node's data is encrypted</h1>
      <p class="subtitle">
        Enter your passphrase to unlock your contacts, groups, and chat history.
      </p>

      <label class="field">
        <input
          ref="passphraseInput"
          v-model="passphrase"
          type="password"
          placeholder="Passphrase"
          autocomplete="current-password"
          :disabled="busy"
          @keyup.enter="unlock"
        />
      </label>

      <p v-if="error" class="error">{{ error }}</p>

      <button
        class="unlock"
        :disabled="busy || passphrase.length === 0"
        @click="unlock"
      >
        {{ busy ? "Unlocking…" : "Unlock" }}
      </button>

      <div class="reset">
        <button v-if="!showReset" class="reset-link" @click="revealReset">
          I've forgotten my passphrase
        </button>

        <div v-else class="reset-panel">
          <p class="reset-warning">
            There is no way to recover your data without the passphrase. The only
            way forward is to delete it and start over: every contact, group, and
            message on this node will be permanently erased. Your node identity is
            kept, so contacts who saved you will still recognise this node.
          </p>

          <label class="field">
            <span class="reset-label">Type <strong>yes</strong> to confirm</span>
            <input
              ref="resetInput"
              v-model="resetConfirmation"
              type="text"
              autocomplete="off"
              :disabled="busy"
            />
          </label>

          <div class="reset-actions">
            <button class="cancel" :disabled="busy" @click="showReset = false">
              Cancel
            </button>
            <button
              class="delete"
              :disabled="busy || !resetConfirmed()"
              @click="emit('reset')"
            >
              Delete all data and start over
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.screen {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  padding: 24px;
  background-color: var(--bg-sunken);
}

.card {
  display: flex;
  flex-direction: column;
  width: 100%;
  max-width: 380px;
  padding: 24px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background-color: var(--bg);
  box-shadow: var(--shadow);
  text-align: center;
}

.lock {
  align-self: center;
  color: var(--text-faint);
}

.title {
  margin: 12px 0 4px;
  font-size: 16px;
  font-weight: 600;
}

.subtitle {
  margin: 0 0 16px;
  font-size: 13px;
  color: var(--text-muted);
}

.field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  text-align: left;
}

.field input {
  width: 100%;
  padding: 9px 11px;
}

.error {
  margin: 8px 0 0;
  font-size: 12px;
  color: var(--danger);
}

.unlock {
  margin-top: 12px;
  padding: 9px 14px;
  border-radius: var(--radius-sm);
  background-color: var(--accent);
  color: var(--accent-contrast);
  font-weight: 500;
}

.unlock:hover:not(:disabled) {
  background-color: var(--accent-hover);
}

.reset {
  margin-top: 18px;
  padding-top: 16px;
  border-top: 1px solid var(--border);
}

.reset-link {
  font-size: 12px;
  color: var(--text-faint);
  text-decoration: underline;
}

.reset-link:hover {
  color: var(--text-muted);
}

.reset-panel {
  display: flex;
  flex-direction: column;
  gap: 10px;
  text-align: left;
}

.reset-warning {
  margin: 0;
  padding: 9px 11px;
  border: 1px solid var(--danger);
  border-radius: var(--radius-sm);
  background-color: var(--danger-bg);
  color: var(--danger);
  font-size: 12px;
}

.reset-label {
  font-size: 12px;
  color: var(--text-muted);
}

.reset-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.cancel,
.delete {
  padding: 7px 12px;
  border-radius: var(--radius-sm);
  font-size: 13px;
  font-weight: 500;
}

.cancel {
  border-color: var(--border-strong);
  color: var(--text);
}

.cancel:hover:not(:disabled) {
  background-color: var(--bg-hover);
}

.delete {
  background-color: var(--danger-solid);
  color: var(--danger-solid-text);
}

.delete:hover:not(:disabled) {
  filter: brightness(1.08);
}
</style>
