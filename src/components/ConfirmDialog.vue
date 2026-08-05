<script setup lang="ts">
/**
 * Modal confirmation for actions that can't be undone.
 *
 * Deliberately awkward to confirm by accident: the cancel button takes focus on
 * open, Escape and a backdrop click both cancel, and the confirm button is the
 * only thing styled as destructive.
 */
import { onMounted, onUnmounted, ref } from "vue";

defineProps<{
  title: string;
  /** What will happen. Kept to one or two sentences. */
  message: string;
  /** The consequence the user most needs to notice, shown as a callout. */
  warning?: string;
  confirmLabel: string;
}>();

const emit = defineEmits<{
  confirm: [];
  cancel: [];
}>();

const cancelButton = ref<HTMLButtonElement | null>(null);

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    emit("cancel");
  }
}

onMounted(() => {
  cancelButton.value?.focus();
  window.addEventListener("keydown", onKeydown);
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown);
});
</script>

<template>
  <Teleport to="body">
    <div class="backdrop" @click.self="emit('cancel')">
      <div class="dialog" role="alertdialog" aria-modal="true">
        <h2 class="title">{{ title }}</h2>
        <p class="message">{{ message }}</p>

        <p v-if="warning" class="warning">
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
          <span>{{ warning }}</span>
        </p>

        <div class="actions">
          <button ref="cancelButton" class="cancel" @click="emit('cancel')">
            Cancel
          </button>
          <button class="confirm" @click="emit('confirm')">
            {{ confirmLabel }}
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
  width: 100%;
  max-width: 400px;
  padding: 18px;
  border: 1px solid var(--border-strong);
  border-radius: var(--radius);
  background-color: var(--bg);
  box-shadow: 0 12px 32px rgba(8, 11, 15, 0.28);
}

.title {
  margin: 0 0 6px;
  font-size: 15px;
  font-weight: 600;
}

.message {
  margin: 0;
  color: var(--text-muted);
}

.warning {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin: 12px 0 0;
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

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 18px;
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
  background-color: var(--danger-solid);
  color: var(--danger-solid-text);
}

.confirm:hover {
  filter: brightness(1.08);
}
</style>
