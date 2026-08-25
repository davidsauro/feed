<script setup lang="ts">
/**
 * Confirming who somebody is before adding them.
 *
 * Adding used to be one click on a name that peer announced about itself. That
 * name is a claim: anybody on the network can send any name they like, so
 * somebody could call themselves John, appear in the list looking like John, and
 * be added as John.
 *
 * The code shown here cannot be claimed. It is derived from the key that peer
 * has to hold to complete a connection at all, so the only way to match somebody
 * else's is to hold their key. Reading it off their screen is the check.
 */
import { onMounted, onUnmounted, ref } from "vue";
import FingerprintBlock from "./FingerprintBlock.vue";

const props = defineProps<{
  peerId: string;
  /**
   * What that peer says it is called, empty when it has said nothing.
   *
   * A claim, not an identity. Somebody added by typed address has made no
   * claim at all, which is a different sentence.
   */
  claimedName: string;
  /** What to put in the nickname box to start with. */
  suggested: string;
  /** The peer's fingerprint, in groups, or null while it is being worked out. */
  fingerprint: string | null;
  /**
   * A contact already using this name.
   *
   * Set when somebody with this name is in the list under a different id, which
   * is what an impersonation attempt looks like from here.
   */
  nameClash: string | null;
}>();

const emit = defineEmits<{
  confirm: [nickname: string];
  cancel: [];
}>();

const nickname = ref(props.suggested);
const nameInput = ref<HTMLInputElement | null>(null);

function confirm() {
  const chosen = nickname.value.trim();

  if (chosen) {
    emit("confirm", chosen);
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    emit("cancel");
  }
}

onMounted(() => {
  nameInput.value?.focus();
  nameInput.value?.select();
  window.addEventListener("keydown", onKeydown);
});

onUnmounted(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <Teleport to="body">
    <div class="backdrop" @click.self="emit('cancel')">
      <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="add-title">
        <h2 id="add-title" class="title">Add this contact?</h2>

        <p v-if="claimedName" class="lead">
          This node says it is called <strong>{{ claimedName }}</strong
          >. Anybody can say that, so check the code below instead.
        </p>

        <p v-else class="lead">
          Nothing about an address says who holds it, so check the code below
          before you trust it.
        </p>

        <!-- The whole point of the dialog. The same component that shows your
             own, so two people comparing two screens are comparing like with
             like. -->
        <FingerprintBlock label="Their code" :code="fingerprint ?? ''" />

        <p class="instruction">
          Ask them to open <strong>Settings</strong> on their device and read out
          their own code. Add them only if it matches, character for character.
        </p>

        <p class="caution">
          Nicknames are not unique and anybody can claim any of them. The code is
          the only part that cannot be.
        </p>

        <!-- Somebody already using this name, under a different key. -->
        <p v-if="nameClash" class="clash">
          You already have a contact called <strong>{{ nameClash }}</strong> with
          a different code. One of these two is not who it says it is.
        </p>

        <label class="field">
          <span class="field-label">Call them</span>
          <input
            ref="nameInput"
            v-model="nickname"
            type="text"
            maxlength="32"
            @keyup.enter="confirm"
          />
        </label>

        <details class="full">
          <summary>Show the full address</summary>
          <code class="peer-id">{{ peerId }}</code>
        </details>

        <div class="actions">
          <button class="secondary" @click="emit('cancel')">Cancel</button>
          <button class="primary" :disabled="!nickname.trim()" @click="confirm">
            Add contact
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.backdrop {
  position: fixed;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background-color: rgba(8, 11, 16, 0.5);
  z-index: 50;
}

.dialog {
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: min(440px, 100%);
  max-height: 100%;
  overflow-y: auto;
  padding: 20px;
  border-radius: var(--radius);
  background-color: var(--bg);
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.28);
}

.title {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
}

.lead,
.instruction,
.caution,
.clash {
  margin: 0;
  font-size: 13px;
  color: var(--text-muted);
}

/* The code is the reason this dialog exists, so it is the thing the eye lands
   on rather than another line of prose. */
.caution {
  color: var(--text-faint);
}

/* Somebody already holding this name is the shape an impersonation takes, so it
   is said loudly rather than left for the reader to spot. */
.clash {
  padding: 9px 11px;
  border: 1px solid var(--danger);
  border-radius: var(--radius-sm);
  background-color: var(--danger-bg);
  color: var(--danger);
}

.field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.field-label {
  font-size: 12px;
  font-weight: 500;
  color: var(--text-muted);
}

/* The address is still there for anyone who wants it, just not in the way of
   the thing worth reading. */
.full {
  font-size: 12px;
  color: var(--text-faint);
}

.full summary {
  cursor: pointer;
}

.peer-id {
  display: block;
  margin-top: 6px;
  font-family: var(--font-mono);
  font-size: 11px;
  overflow-wrap: anywhere;
}

.actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  margin-top: 4px;
}

.secondary {
  padding: 8px 14px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  font-weight: 500;
  color: var(--text);
}

.secondary:hover {
  background-color: var(--bg-hover);
}

.primary {
  padding: 8px 14px;
  border-radius: var(--radius-sm);
  background-color: var(--accent);
  font-weight: 500;
  color: var(--accent-contrast);
}

.primary:hover:not(:disabled) {
  background-color: var(--accent-hover);
}
</style>
