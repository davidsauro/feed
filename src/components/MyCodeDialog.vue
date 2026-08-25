<script setup lang="ts">
/**
 * This node's own code, for reading out to somebody adding you.
 *
 * Reached by tapping your own name, because that is where somebody looks for
 * something about themselves, and because being asked to read this out happens
 * often enough that it should not be behind Settings.
 */
import { onMounted, onUnmounted, ref } from "vue";
import FingerprintBlock from "./FingerprintBlock.vue";

const props = defineProps<{
  peerId: string;
  fingerprint: string;
  /** The name this node asks others to call it, empty if unset. */
  name: string;
}>();

const emit = defineEmits<{ close: [] }>();

const copied = ref<"code" | "address" | null>(null);
const closeButton = ref<HTMLButtonElement | null>(null);

async function copy(what: "code" | "address") {
  try {
    await navigator.clipboard.writeText(
      what === "code" ? props.fingerprint : props.peerId,
    );
  } catch {
    // Nothing useful to do without a clipboard. Both are selectable by hand.
    return;
  }

  copied.value = what;
  window.setTimeout(() => {
    copied.value = null;
  }, 1500);
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    emit("close");
  }
}

onMounted(() => {
  closeButton.value?.focus();
  window.addEventListener("keydown", onKeydown);
});

onUnmounted(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <Teleport to="body">
    <div class="backdrop" @click.self="emit('close')">
      <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="my-code-title">
        <h2 id="my-code-title" class="title">
          {{ name ? `You, as ${name}` : "You" }}
        </h2>

        <FingerprintBlock label="Your code" :code="fingerprint" />

        <p class="lead">
          Read this out to somebody adding you, and they can check it matches
          what their screen shows. It comes from your key, so nobody else can
          have it. A name is not like that.
        </p>

        <details class="full">
          <summary>Show your full address</summary>
          <code class="peer-id">{{ peerId }}</code>
          <p class="hint">
            What somebody needs to add you when they are not on this network.
          </p>
        </details>

        <div class="actions">
          <button class="secondary" @click="copy('address')">
            {{ copied === "address" ? "Copied" : "Copy address" }}
          </button>
          <button class="secondary" @click="copy('code')">
            {{ copied === "code" ? "Copied" : "Copy code" }}
          </button>
          <button ref="closeButton" class="primary" @click="emit('close')">Done</button>
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
  width: min(420px, 100%);
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
.hint {
  margin: 0;
  font-size: 13px;
  color: var(--text-muted);
}

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
  color: var(--text);
  overflow-wrap: anywhere;
}

.hint {
  margin-top: 4px;
  font-size: 11px;
}

.actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  margin-top: 4px;
}

.secondary {
  padding: 8px 12px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  font-size: 13px;
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

.primary:hover {
  background-color: var(--accent-hover);
}
</style>
