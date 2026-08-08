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
  /**
   * Whether this has been looked at since it arrived.
   *
   * Only meaningful on an incoming file. Nothing arrives with a prompt, so this
   * is what lets the Files tab say something turned up while you were elsewhere.
   */
  seen: boolean;
}

/**
 * A file that has been picked but not sent.
 *
 * Mirrors the `PickedFile` struct in Rust. Picking and sending are separate
 * steps, so a batch can be assembled and looked over before any of it leaves.
 */
export interface PickedFile {
  path: string;
  name: string;
  size: number;
  /** Over the size ceiling, so it is refused here rather than at send time. */
  too_large: boolean;
  /** Could not be read at all. A folder lands here too. */
  unreadable: boolean;
}

/** Whether a picked file can actually be sent. */
export function canSend(file: PickedFile): boolean {
  return !file.too_large && !file.unreadable;
}

/** Why a picked file is not going anywhere, or null when it is fine. */
export function describeProblem(file: PickedFile): string | null {
  if (file.unreadable) {
    return "could not be read";
  }

  if (file.too_large) {
    return "too large to send";
  }

  return null;
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

/**
 * A timestamp as something worth reading in a list.
 *
 * Today shows only the time, because the date would be noise. Anything older
 * carries its date, because "14:32" on its own is useless a week later.
 */
export function describeWhen(epochMs: number): string {
  const when = new Date(epochMs);
  const time = when.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });

  if (new Date().toDateString() === when.toDateString()) {
    return time;
  }

  const date = when.toLocaleDateString(undefined, { day: "numeric", month: "short" });

  return `${date} ${time}`;
}
