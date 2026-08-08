/** Types shared between App.vue and the components it renders. */

/** A peer the user has given a name to. Mirrors the `Contact` struct in Rust. */
export interface Contact {
  peer_id: string;
  nickname: string;
}

/**
 * A group conversation. Mirrors the `Group` struct in Rust.
 *
 * `members` includes us. Whoever created the group decides who's in it, and the
 * list travels with every message so members stay in step.
 */
export interface Group {
  id: string;
  name: string;
  members: string[];
}

/**
 * How far along an outgoing message is.
 *
 * - `sending`: handed to the network, not yet acknowledged.
 * - `delivered`: the other node accepted it. For a group this means it reached
 *   the mesh — gossipsub can't tell us who read it, so group messages stop here.
 * - `read`: the other node has the conversation open and saw it.
 * - `failed`: it didn't go out. Nobody was listening, or the send errored.
 */
export type MessageStatus = "sending" | "delivered" | "read" | "failed";

/** One message in a conversation. Mirrors the `ChatMessage` struct in Rust. */
export interface ChatMessage {
  id: string;
  sender: string;
  text: string;
  status: MessageStatus;
  /**
   * When the sender says they wrote it, in milliseconds since the epoch.
   *
   * Conversations are ordered by this rather than by arrival, because messages
   * can now take different routes and arrive out of order. It travels inside the
   * sealed payload, so nothing carrying the message can alter it.
   */
  sent_at: number;
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

/**
 * One file, in one direction, between us and one other person.
 *
 * Mirrors the `FileTransfer` struct in Rust. `transferred` is what a progress
 * bar reads, and what lets a broken transfer pick up where it stopped.
 */
export interface FileTransfer {
  id: string;
  peer_id: string;
  direction: "outgoing" | "incoming";
  name: string;
  size: number;
  hash: string;
  key: string;
  path: string | null;
  status: "offered" | "pending" | "transferring" | "complete" | "failed";
  transferred: number;
  error: string | null;
  sent_at: number;
}

/** Bytes as something a person reads. */
export function describeSize(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }

  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(0)} KB`;
  }

  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
