<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

const peerId = ref("");
const errorMessage = ref("");

async function loadIdentity() {
  try {
    errorMessage.value = "";
    // Call the updated Rust function 'get_identity'
    peerId.value = await invoke("get_identity");
  } catch (error) {
    // If Rust returns an Err(), it gets caught here
    errorMessage.value = `Error loading identity: ${error}`;
  }
}
</script>

<template>
  <main class="container">
    <h1>P2P Mesh Node</h1>
    
    <div class="row">
      <button @click="loadIdentity">Load Identity</button>
    </div>

    <p v-if="peerId" class="success">{{ peerId }}</p>
    <p v-if="errorMessage" class="error">{{ errorMessage }}</p>
  </main>
</template>

<style scoped>
.container {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100vh;
  font-family: sans-serif;
}
.row { margin: 20px 0; }
button { padding: 10px 20px; font-size: 16px; cursor: pointer; }
.success { color: green; font-family: monospace; font-size: 1.1em; }
.error { color: red; }
</style>