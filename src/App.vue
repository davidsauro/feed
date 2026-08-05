<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const myPeerId = ref("");
const errorMessage = ref("");
const nicknameInput = ref("");
const nodeInstance = ref("Loading...");

const activePeers = ref<Set<string>>(new Set());
const savedContacts = ref<{ peer_id: string, nickname: string }[]>([]);

const editingContact = ref<string | null>(null);
const editNicknameInput = ref("");

interface ChatMessage {
  id: string;
  sender: string;
  text: string;
  status: 'sending' | 'delivered' | 'read';
}

const selectedContact = ref<{ peer_id: string, nickname: string } | null>(null);
const chatInput = ref("");
const messages = ref<Record<string, ChatMessage[]>>({});
const unreadStatus = ref<Record<string, boolean>>({});

const unregisteredActivePeers = computed(() => {
  return Array.from(activePeers.value).filter(
    (peer) => !savedContacts.value.some((contact) => contact.peer_id === peer)
  );
});

const currentMessages = computed(() => {
  if (!selectedContact.value) return [];
  return messages.value[selectedContact.value.peer_id] || [];
});

async function loadIdentity() {
  try { myPeerId.value = await invoke("get_identity"); } 
  catch (e) { errorMessage.value = `Error: ${e}`; }
}

async function loadContacts() {
  try { 
    savedContacts.value = await invoke("get_contacts"); 
    
    // Check database for any unread messages from these contacts
    for (const contact of savedContacts.value) {
      const history: ChatMessage[] = await invoke("get_chat_history", { peerId: contact.peer_id });
      const hasUnread = history.some(m => m.sender === contact.peer_id && m.status !== 'read');
      unreadStatus.value[contact.peer_id] = hasUnread;
    }
  } 
  catch (e) { console.error("Failed to load contacts", e); }
}

async function saveContact(peerId: string) {
  if (!nicknameInput.value.trim()) return alert("Nickname is required!");
  try {
    await invoke("save_contact", { peerId: peerId, nickname: nicknameInput.value.trim() });
    nicknameInput.value = "";
    await loadContacts();
  } catch (e) { alert(`Error saving contact: ${e}`); }
}

function startEdit(contact: { peer_id: string, nickname: string }) {
  editingContact.value = contact.peer_id;
  editNicknameInput.value = contact.nickname;
}

async function saveEdit(peerId: string) {
  if (!editNicknameInput.value.trim()) return;
  try {
    await invoke("save_contact", { peerId, nickname: editNicknameInput.value.trim() });
    editingContact.value = null;
    await loadContacts();
    if (selectedContact.value && selectedContact.value.peer_id === peerId) {
      selectedContact.value.nickname = editNicknameInput.value.trim();
    }
  } catch (e) { alert(`Error saving edit: ${e}`); }
}

async function sendReadReceipt(peerId: string, ids: string[]) {
  const payload = JSON.stringify({ type: 'read', messageIds: ids });
  try { await invoke("send_message", { peerId, message: payload }); } catch(e) {}
}

async function selectContact(contact: { peer_id: string, nickname: string }) {
  selectedContact.value = contact;
  
  // Load history from SQLite
  messages.value[contact.peer_id] = await invoke("get_chat_history", { peerId: contact.peer_id });
  
  // Process Unreads
  if (unreadStatus.value[contact.peer_id]) {
    unreadStatus.value[contact.peer_id] = false;
  }
  
  const unreadMsgs = messages.value[contact.peer_id].filter(m => m.sender === contact.peer_id && m.status !== 'read');
  if (unreadMsgs.length > 0) {
    const unreadIds = unreadMsgs.map(m => m.id);
    sendReadReceipt(contact.peer_id, unreadIds);
    
    // Mark as read in our local UI and Database
    unreadMsgs.forEach(m => {
      m.status = 'read';
      invoke("update_message_status", { id: m.id, status: 'read' }).catch(()=>{});
    });
  }
}

async function sendMessage() {
  if (!chatInput.value.trim() || !selectedContact.value) return;
  const text = chatInput.value.trim();
  const peerId = selectedContact.value.peer_id;
  
  const id = crypto.randomUUID();
  const payload = JSON.stringify({ type: 'chat', id, text });
  
  if (!messages.value[peerId]) messages.value[peerId] = [];
  const msgObj: ChatMessage = { id, sender: myPeerId.value, text, status: 'sending' };
  messages.value[peerId].push(msgObj);
  chatInput.value = "";
  
  // Save to SQLite
  await invoke("save_chat_message", { id, peerId, sender: myPeerId.value, text, status: 'sending' });
  
  try {
    await invoke("send_message", { peerId, message: payload });
    msgObj.status = 'delivered';
    await invoke("update_message_status", { id, status: 'delivered' });
  } catch (e) {
    alert("Failed to send: " + e);
  }
}

onMounted(async () => {
  nodeInstance.value = await invoke("get_node_id");
  await loadIdentity();
  await loadContacts();

  try {
    const initialPeers = await invoke<string[]>("get_active_peers");
    initialPeers.forEach(p => activePeers.value.add(p));
  } catch(e) {}

  await listen<string>("peer-discovered", (event) => activePeers.value.add(event.payload));
  await listen<string>("peer-lost", (event) => activePeers.value.delete(event.payload));

  await listen<{ sender: string, message: string }>("chat-received", async (event) => {
    const { sender, message } = event.payload;
    try {
      const data = JSON.parse(message);
      
      if (data.type === 'chat') {
        
        const msgObj = { 
          id: data.id, 
          sender, 
          text: data.text, 
          status: 'delivered' 
        } as ChatMessage;
        
        if (!messages.value[sender]) messages.value[sender] = [];
        messages.value[sender].push(msgObj);
        
        // Save received message to SQLite
        await invoke("save_chat_message", { id: data.id, peerId: sender, sender, text: data.text, status: 'delivered' });
        
        if (selectedContact.value?.peer_id === sender) {
          sendReadReceipt(sender, [data.id]);
          msgObj.status = 'read';
          await invoke("update_message_status", { id: data.id, status: 'read' });
        } else {
          unreadStatus.value[sender] = true;
        }
      } 
      else if (data.type === 'read') {
        if (messages.value[sender]) {
          for (const msg of messages.value[sender]) {
            if (data.messageIds.includes(msg.id)) {
              msg.status = 'read';
              invoke("update_message_status", { id: msg.id, status: 'read' }).catch(()=>{});
            }
          }
        }
      }
    } catch (e) { }
  });
});
</script>

<template>
  <main class="container">
    <h1>P2P Mesh Node</h1>
    <p v-if="myPeerId" class="success">My ID: {{ myPeerId }}</p>
    <p v-if="errorMessage" class="error">{{ errorMessage }}</p>

    <div class="grid">
      <div class="card">
        <h2>Discovered</h2>
        <ul v-if="unregisteredActivePeers.length > 0">
          <li v-for="peer in unregisteredActivePeers" :key="peer">
            <span class="peer-id">{{ peer }}</span>
            <div class="action-row">
              <input v-model="nicknameInput" placeholder="Required..." />
              <button @click="saveContact(peer)">Save</button>
            </div>
          </li>
        </ul>
        <p v-else>No unregistered peers.</p>
      </div>

      <div class="card">
        <h2>Contacts</h2>
        <ul v-if="savedContacts.length > 0">
          <li v-for="contact in savedContacts" :key="contact.peer_id" 
              @click="selectContact(contact)"
              class="contact-item" :class="{ selected: selectedContact?.peer_id === contact.peer_id }">
            
            <!-- Normal View -->
            <div class="contact-header" v-if="editingContact !== contact.peer_id">
              <span class="status-dot" :class="{ online: activePeers.has(contact.peer_id) }"></span>
              <strong>{{ contact.nickname }}</strong>
              
              <!-- SVG Unread Icon -->
              <span v-if="unreadStatus[contact.peer_id]" class="unread-icon" title="New Message">
                <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none">
                  <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
                </svg>
              </span>

              <button class="icon-btn edit-btn" @click.stop="startEdit(contact)">✎</button>
            </div>

            <!-- Edit View -->
            <div class="edit-row" v-else @click.stop>
              <input v-model="editNicknameInput" @keyup.enter="saveEdit(contact.peer_id)" />
              <button @click="saveEdit(contact.peer_id)">✓</button>
              <button @click="editingContact = null">✕</button>
            </div>

            <div class="peer-id-small">{{ contact.peer_id }}</div>
          </li>
        </ul>
        <p v-else>No contacts.</p>
      </div>

      <div class="card chat-panel" v-if="selectedContact">
        <h2>Chat: {{ selectedContact.nickname }}</h2>
        <div class="message-history">
          <div v-for="msg in currentMessages" :key="msg.id"
               :class="['message', msg.sender === myPeerId ? 'sent' : 'received']">
            <div class="msg-text">{{ msg.text }}</div>
            <div class="msg-meta" v-if="msg.sender === myPeerId">
              
              <!-- SVG Clock (Sending) -->
              <span v-if="msg.status === 'sending'" class="status-icon">
                <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2.5" fill="none">
                  <circle cx="12" cy="12" r="10"></circle>
                  <polyline points="12 6 12 12 16 14"></polyline>
                </svg>
              </span>

              <!-- SVG Single Check (Delivered) -->
              <span v-if="msg.status === 'delivered'" class="status-icon">
                <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="3" fill="none">
                  <polyline points="20 6 9 17 4 12"></polyline>
                </svg>
              </span>

              <!-- SVG Double Check (Read) -->
              <span v-if="msg.status === 'read'" class="status-icon">
                <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="3" fill="none">
                  <polyline points="18 6 7 17 2 12"></polyline>
                  <polyline points="22 6 12 16 11 15"></polyline>
                </svg>
              </span>

            </div>
          </div>
        </div>
        <div class="action-row">
          <input v-model="chatInput" placeholder="Message..." @keyup.enter="sendMessage" />
          <button @click="sendMessage">Send</button>
        </div>
      </div>
    </div>
    
    <div class="port-indicator">Instance: Node {{ nodeInstance }}</div>
  </main>
</template>

<style scoped>
.container { font-family: sans-serif; padding: 20px; max-width: 1000px; margin: 0 auto; }
.success { color: green; font-family: monospace; font-size: 1.1em; text-align: center; }
.error { color: red; text-align: center; }
.grid { display: flex; gap: 20px; margin-top: 20px; align-items: stretch; }
.card { flex: 1; border: 1px solid #ccc; padding: 15px; border-radius: 8px; display: flex; flex-direction: column; }
ul { list-style: none; padding: 0; margin: 0; }
li { padding: 10px 0; border-bottom: 1px solid #eee; }

.contact-item { cursor: pointer; padding: 10px; border-radius: 4px; transition: background 0.2s; position: relative; }
.contact-item:hover { background-color: #f5f5f5; }
.contact-item.selected { background-color: #e3f2fd; border: 1px solid #90caf9; }

.peer-id { font-family: monospace; font-size: 0.8em; word-break: break-all; color: #555; }
.peer-id-small { font-family: monospace; font-size: 0.7em; color: #888; margin-top: 4px; }
.action-row { display: flex; gap: 10px; margin-top: 8px; }
input { flex: 1; padding: 5px; }
button { padding: 5px 10px; cursor: pointer; }

/* Contact Header & Icons */
.contact-header { display: flex; align-items: center; gap: 8px; }
.status-dot { width: 10px; height: 10px; border-radius: 50%; background-color: #ff4444; display: inline-block; }
.status-dot.online { background-color: #00C851; }
.unread-icon { font-size: 0.9em; margin-left: 5px; }
.icon-btn { background: none; border: none; font-size: 1.2em; padding: 0; color: #666; opacity: 0; transition: opacity 0.2s; }
.contact-item:hover .icon-btn { opacity: 1; }
.icon-btn:hover { color: #000; }
.edit-row { display: flex; gap: 5px; align-items: center; width: 100%; }

/* Chat Styling */
.message-history { flex: 1; overflow-y: auto; padding: 10px; border: 1px solid #eee; border-radius: 4px; margin-bottom: 10px; min-height: 250px; display: flex; flex-direction: column; gap: 8px; }
.message { padding: 8px 12px; border-radius: 12px; max-width: 80%; width: fit-content; position: relative; }
.message.sent { background-color: #007bff; color: white; align-self: flex-end; padding-bottom: 18px; }
.message.received { background-color: #e9ecef; color: black; align-self: flex-start; }
.msg-text { word-wrap: break-word; }
.msg-meta { position: absolute; bottom: 4px; right: 8px; font-size: 0.65em; opacity: 0.8; letter-spacing: 1px; }

.port-indicator { position: fixed; bottom: 15px; right: 15px; background-color: #333; color: #fff; padding: 6px 12px; border-radius: 20px; font-family: monospace; font-size: 0.85em; opacity: 0.8; }

.unread-icon { display: flex; align-items: center; color: #007bff; margin-left: auto; padding-right: 5px; }
.status-icon { display: flex; align-items: center; justify-content: center; opacity: 0.9; }
.msg-meta { display: flex; align-items: center; justify-content: flex-end; position: absolute; bottom: 4px; right: 8px; }

</style>

