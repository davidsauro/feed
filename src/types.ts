/** Types shared between App.vue and the components it renders. */

/** A peer the user has given a name to. Mirrors the `Contact` struct in Rust. */
export interface Contact {
  peer_id: string;
  nickname: string;
}

/**
 * How far along an outgoing message is.
 *
 * - `sending`: handed to the network, not yet acknowledged.
 * - `delivered`: the other node accepted it.
 * - `read`: the other node has the conversation open and saw it.
 */
export type MessageStatus = "sending" | "delivered" | "read";

/** One message in a conversation. Mirrors the `ChatMessage` struct in Rust. */
export interface ChatMessage {
  id: string;
  sender: string;
  text: string;
  status: MessageStatus;
}

/**
 * Shortens a peer ID for display.
 *
 * Peer IDs are 52 characters, which is far too wide for a sidebar row. The head
 * and tail are enough to tell two peers apart at a glance, and the full value
 * is available in a tooltip wherever this is used.
 */
export function shortPeerId(peerId: string): string {
  if (peerId.length <= 16) {
    return peerId;
  }

  return `${peerId.slice(0, 8)}…${peerId.slice(-6)}`;
}
