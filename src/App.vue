<script setup lang="ts">
import { ref, onMounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const myPeerId = ref("");
const errorMessage = ref("");
const nicknameInput = ref("");

// State for network and DB
const activePeers = ref<Set<String>>(new Set());
const savedContacts = ref<{ peer_id: string, nickname: string }[]>([]);

async function loadIdentity() {
  try {
    myPeerId.value = await invoke("get_identity");
  } catch (e) {
    errorMessage.value = `Error loading identity: ${e}`;
  }
}

async function loadContacts() {
  try {
    savedContacts.value = await invoke("get_contacts");
  } catch (e) {
    console.error("Failed to load contacts", e);
  }
}

async function saveContact(peerId: string) {
  if (!nicknameInput.value) return alert("Enter a nickname!");
  
  try {
    await invoke("save_contact", { peerId: peerId, nickname: nicknameInput.value });
    nicknameInput.value = ""; // clear input
    await loadContacts(); // refresh the list
  } catch (e) {
    alert(`Error saving contact: ${e}`);
  }
}

onMounted(async () => {
  await loadIdentity();
  await loadContacts();

  // Listen for real-time events from Rust
  await listen<string>("peer-discovered", (event) => {
    activePeers.value.add(event.payload);
  });

  await listen<string>("peer-lost", (event) => {
    activePeers.value.delete(event.payload);
  });
});
</script>

<template>
  <main class="container">
    <h1>P2P Mesh Node</h1>
    <p v-if="myPeerId" class="success">My ID: {{ myPeerId }}</p>
    <p v-if="errorMessage" class="error">{{ errorMessage }}</p>

    <div class="grid">
      <!-- Active Network Peers -->
      <div class="card">
        <h2>Discovered Peers</h2>
        <ul v-if="activePeers.size > 0">
          <li v-for="peer in activePeers" :key="peer.toString()">
            <span class="peer-id">{{ peer }}</span>
            <div class="action-row">
              <input v-model="nicknameInput" placeholder="Nickname..." />
              <button @click="saveContact(peer.toString())">Save</button>
            </div>
          </li>
        </ul>
        <p v-else>Listening for peers on local network...</p>
      </div>

      <!-- Saved SQLite Contacts -->
      <div class="card">
        <h2>Address Book</h2>
        <ul v-if="savedContacts.length > 0">
          <li v-for="contact in savedContacts" :key="contact.peer_id">
            <strong>{{ contact.nickname }}</strong>
            <div class="peer-id-small">{{ contact.peer_id }}</div>
          </li>
        </ul>
        <p v-else>No contacts saved yet.</p>
      </div>
    </div>
  </main>
</template>

<style scoped>
.container { font-family: sans-serif; padding: 20px; max-width: 800px; margin: 0 auto; }
.success { color: green; font-family: monospace; font-size: 1.1em; text-align: center; }
.error { color: red; text-align: center; }
.grid { display: flex; gap: 20px; margin-top: 20px; }
.card { flex: 1; border: 1px solid #ccc; padding: 15px; border-radius: 8px; }
ul { list-style: none; padding: 0; }
li { padding: 10px 0; border-bottom: 1px solid #eee; }
.peer-id { font-family: monospace; font-size: 0.8em; word-break: break-all; color: #555; }
.peer-id-small { font-family: monospace; font-size: 0.7em; color: #888; }
.action-row { display: flex; gap: 10px; margin-top: 8px; }
input { flex: 1; padding: 5px; }
button { padding: 5px 10px; cursor: pointer; }
</style>