//! Moving the bytes of a file between two nodes.
//!
//! The offer travels as an ordinary sealed message, so the recipient learns a
//! file exists, what it is called, how big it is, and the key to open it, all
//! before a single byte moves. This module is what moves the bytes afterwards.
//!
//! # The receiver pulls
//!
//! It would be equally possible for the sender to push, and pulling is better
//! for one reason: only the receiver knows how much it already has. A transfer
//! that died half way resumes by asking for the rest, with no negotiation and
//! nothing for the two sides to disagree about.
//!
//! # Framing
//!
//! Every message on the stream is a four byte length followed by that many
//! bytes. The receiver could in principle work out each chunk's size from the
//! file size, since all but the last are full, but four bytes per 64KB chunk
//! costs nothing and means a bug shows up as a failed read rather than as
//! silently misaligned data.
//!
//! # What the sender checks
//!
//! A pull names a transfer by an id the sender chose at random and sent inside
//! an encrypted message. Guessing one is not feasible, but the sender checks
//! anyway that the transfer was offered to the peer now asking for it, so a
//! contact cannot fetch a file meant for somebody else even if an id leaks.

use std::path::{Path, PathBuf};

use futures::{AsyncReadExt, AsyncWriteExt};
use libp2p::{PeerId, Stream, StreamProtocol};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt as TokioReadExt, AsyncSeekExt, AsyncWriteExt as TokioWriteExt};

use crate::file_crypto::{self, FileKey, CHUNK_SIZE, SEALED_CHUNK_SIZE};

/// The protocol a file travels over. Separate from everything else so that a
/// file transfer cannot interfere with, or be delayed behind, conversation.
pub const FILE_PROTOCOL: StreamProtocol = StreamProtocol::new("/file/1.0.0");

/// A request is small. Anything larger is a peer doing something odd.
const MAX_REQUEST_SIZE: usize = 4 * 1024;

/// What the receiver asks for.
#[derive(Serialize, Deserialize)]
struct PullRequest {
    transfer_id: String,
    /// Where to start. Zero for a new transfer, higher when resuming.
    from_chunk: u64,
}

/// What the sender says before the bytes start.
#[derive(Serialize, Deserialize)]
struct PullResponse {
    ok: bool,
    error: Option<String>,
}

/// Progress, sent to the interface as `file-progress`.
#[derive(Clone, Serialize)]
struct Progress {
    transfer_id: String,
    transferred: u64,
    size: u64,
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

async fn write_frame<S>(stream: &mut S, payload: &[u8]) -> Result<(), String>
where
    S: futures::AsyncWrite + Unpin,
{
    let length = u32::try_from(payload.len()).map_err(|_| "frame too large".to_string())?;

    stream
        .write_all(&length.to_be_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.write_all(payload).await.map_err(|e| e.to_string())?;

    Ok(())
}

async fn read_frame<S>(stream: &mut S, limit: usize) -> Result<Vec<u8>, String>
where
    S: futures::AsyncRead + Unpin,
{
    let mut length = [0u8; 4];
    stream
        .read_exact(&mut length)
        .await
        .map_err(|e| e.to_string())?;

    let length = u32::from_be_bytes(length) as usize;
    if length > limit {
        return Err(format!("a frame of {} bytes is larger than allowed", length));
    }

    let mut payload = vec![0u8; length];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(|e| e.to_string())?;

    Ok(payload)
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

/// Answers one pull request.
///
/// Runs in its own task, so a large file does not hold up anything else.
pub async fn serve(app: AppHandle, peer: PeerId, mut stream: Stream) {
    // Read first, so that whatever happens next can be recorded against the
    // transfer it happened to.
    let request = match read_request(&mut stream).await {
        Ok(request) => request,
        Err(error) => {
            eprintln!("could not read a file request from {}: {}", peer, error);
            let _ = stream.close().await;
            return;
        }
    };

    let id = request.transfer_id.clone();
    let _ = crate::set_file_status(&app, &id, "transferring", None);
    let _ = app.emit("file-changed", &id);

    match serve_inner(&app, peer, &mut stream, request).await {
        Ok(()) => {
            let _ = crate::set_file_status(&app, &id, "complete", None);
        }
        Err(error) => {
            eprintln!("could not send a file to {}: {}", peer, error);

            // Best effort. The other side may already be gone, which is how most
            // of these end.
            let _ = write_frame(
                &mut stream,
                &serde_json::to_vec(&PullResponse {
                    ok: false,
                    error: Some(error.clone()),
                })
                .unwrap_or_default(),
            )
            .await;

            let _ = crate::set_file_status(&app, &id, "failed", Some(&error));
        }
    }

    let _ = app.emit("file-changed", &id);
    let _ = stream.close().await;
}

async fn read_request(stream: &mut Stream) -> Result<PullRequest, String> {
    serde_json::from_slice(&read_frame(stream, MAX_REQUEST_SIZE).await?)
        .map_err(|e| format!("could not read the request: {}", e))
}

async fn serve_inner(
    app: &AppHandle,
    peer: PeerId,
    stream: &mut Stream,
    request: PullRequest,
) -> Result<(), String> {
    let transfer = crate::find_outgoing_file(app, &request.transfer_id, &peer.to_string())?
        .ok_or_else(|| "no such transfer".to_string())?;

    let path = transfer
        .path
        .ok_or_else(|| "that file is no longer on this node".to_string())?;

    let key = FileKey::from_bytes(decode_key(&transfer.key)?);
    let chunks = file_crypto::chunk_count(transfer.size);

    if request.from_chunk > chunks {
        return Err("asked to start beyond the end of the file".to_string());
    }

    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| format!("could not open {}: {}", path, e))?;

    file.seek(std::io::SeekFrom::Start(request.from_chunk * CHUNK_SIZE as u64))
        .await
        .map_err(|e| e.to_string())?;

    write_frame(
        stream,
        &serde_json::to_vec(&PullResponse {
            ok: true,
            error: None,
        })
        .map_err(|e| e.to_string())?,
    )
    .await?;

    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut sent = request.from_chunk * CHUNK_SIZE as u64;

    for index in request.from_chunk..chunks {
        let filled = read_up_to(&mut file, &mut buffer).await?;
        let is_last = index + 1 == chunks;

        let sealed = file_crypto::seal_chunk(&key, index, is_last, &buffer[..filled])?;
        write_frame(stream, &sealed).await?;

        // The sender gets a progress bar for the same reason the receiver does.
        // Watching a file leave is most of knowing that it worked.
        sent += filled as u64;
        crate::set_file_progress(app, &request.transfer_id, sent)?;

        let _ = app.emit(
            "file-progress",
            Progress {
                transfer_id: request.transfer_id.clone(),
                transferred: sent,
                size: transfer.size,
            },
        );
    }

    Ok(())
}

/// Fills the buffer as far as the file allows.
///
/// A single read is not enough: it may return less than asked for, and a chunk
/// that came up short would be sealed as a chunk of that size and arrive as a
/// hole in the middle of the file.
async fn read_up_to(file: &mut tokio::fs::File, buffer: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;

    while filled < buffer.len() {
        let read = file
            .read(&mut buffer[filled..])
            .await
            .map_err(|e| e.to_string())?;

        if read == 0 {
            break;
        }

        filled += read;
    }

    Ok(filled)
}

// ---------------------------------------------------------------------------
// Receiving
// ---------------------------------------------------------------------------

/// Fetches a file that was offered to us, resuming if there is a partial one.
pub async fn fetch(
    app: AppHandle,
    mut control: libp2p_stream::Control,
    peer: PeerId,
    transfer_id: String,
) {
    let outcome = fetch_inner(&app, &mut control, peer, &transfer_id).await;

    match outcome {
        Ok(()) => {
            let _ = crate::set_file_status(&app, &transfer_id, "complete", None);
        }
        Err(error) => {
            eprintln!("could not receive a file from {}: {}", peer, error);
            let _ = crate::set_file_status(&app, &transfer_id, "failed", Some(&error));
        }
    }

    let _ = app.emit("file-changed", &transfer_id);
}

async fn fetch_inner(
    app: &AppHandle,
    control: &mut libp2p_stream::Control,
    peer: PeerId,
    transfer_id: &str,
) -> Result<(), String> {
    let transfer = crate::find_incoming_file(app, transfer_id)?
        .ok_or_else(|| "no such transfer".to_string())?;

    let path = transfer
        .path
        .ok_or_else(|| "this transfer has nowhere to write".to_string())?;

    let key = FileKey::from_bytes(decode_key(&transfer.key)?);
    let chunks = file_crypto::chunk_count(transfer.size);

    // Whole chunks only. A partial chunk is never counted as received, so the
    // file on disk always ends on a boundary and resuming lands exactly right.
    let from_chunk = transfer.transferred / CHUNK_SIZE as u64;
    let mut transferred = from_chunk * CHUNK_SIZE as u64;

    crate::set_file_status(app, transfer_id, "transferring", None)?;

    let mut stream = control
        .open_stream(peer, FILE_PROTOCOL)
        .await
        .map_err(|e| format!("could not reach them: {}", e))?;

    write_frame(
        &mut stream,
        &serde_json::to_vec(&PullRequest {
            transfer_id: transfer_id.to_string(),
            from_chunk,
        })
        .map_err(|e| e.to_string())?,
    )
    .await?;

    let response: PullResponse =
        serde_json::from_slice(&read_frame(&mut stream, MAX_REQUEST_SIZE).await?)
            .map_err(|e| format!("could not read their answer: {}", e))?;

    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "they would not send it".to_string()));
    }

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(from_chunk == 0)
        .open(&path)
        .await
        .map_err(|e| format!("could not write to {}: {}", path, e))?;

    file.seek(std::io::SeekFrom::Start(transferred))
        .await
        .map_err(|e| e.to_string())?;

    for index in from_chunk..chunks {
        let sealed = read_frame(&mut stream, SEALED_CHUNK_SIZE + 64).await?;
        let is_last = index + 1 == chunks;
        let plaintext = file_crypto::open_chunk(&key, index, is_last, &sealed)?;

        file.write_all(&plaintext).await.map_err(|e| e.to_string())?;

        transferred += plaintext.len() as u64;
        crate::set_file_progress(app, transfer_id, transferred)?;

        let _ = app.emit(
            "file-progress",
            Progress {
                transfer_id: transfer_id.to_string(),
                transferred,
                size: transfer.size,
            },
        );
    }

    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);

    // The chunk tags already prove nothing was altered on the way. This catches
    // the other thing: that what the sender read off its own disk was what it
    // meant to send.
    let arrived = hash_file(&path).await?;
    if arrived != transfer.hash {
        return Err("the file that arrived is not the file that was offered".to_string());
    }

    Ok(())
}

/// Hashes a file on disk, a piece at a time.
pub async fn hash_file(path: &str) -> Result<String, String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("could not read {}: {}", path, e))?;

    let mut hash = file_crypto::RunningHash::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];

    loop {
        let read = file.read(&mut buffer).await.map_err(|e| e.to_string())?;

        if read == 0 {
            break;
        }

        hash.update(&buffer[..read]);
    }

    Ok(hash.finish())
}

// ---------------------------------------------------------------------------
// Where files land
// ---------------------------------------------------------------------------

/// Cleans up a name that arrived from somebody else.
///
/// The sender chooses this, so it is not to be trusted with anything. A name
/// containing a path would otherwise let a contact write wherever they liked,
/// which is the one way a file transfer can do real damage to a machine.
pub fn safe_file_name(name: &str) -> String {
    // Anything before a separator is a directory, and none of it is wanted.
    let bare = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('.');

    let cleaned: String = bare
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .take(120)
        .collect();

    if cleaned.is_empty() {
        "file".to_string()
    } else {
        cleaned
    }
}

/// Finds a name in this folder that is not already taken.
pub fn available_path(directory: &Path, name: &str) -> PathBuf {
    let candidate = directory.join(name);

    if !candidate.exists() {
        return candidate;
    }

    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{}", extension)),
        _ => (name, String::new()),
    };

    for attempt in 2..1000 {
        let candidate = directory.join(format!("{} ({}){}", stem, attempt, extension));

        if !candidate.exists() {
            return candidate;
        }
    }

    directory.join(format!("{} ({}){}", stem, "many", extension))
}

/// Turns a contact's name into something usable as a folder.
pub fn folder_for(nickname: &str, peer_id: &str) -> String {
    let cleaned = safe_file_name(nickname);

    if cleaned == "file" {
        // Nothing usable in the nickname, so fall back to something that is
        // always present and always unique.
        return peer_id.chars().take(16).collect();
    }

    cleaned
}

fn decode_key(hex: &str) -> Result<[u8; 32], String> {
    if hex.len() != 64 {
        return Err("that file key is the wrong length".to_string());
    }

    let mut key = [0u8; 32];

    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| "that file key is not readable".to_string())?;
    }

    Ok(key)
}

pub fn encode_key(key: &FileKey) -> String {
    key.as_bytes()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sender picks the file name, so this is the one place a transfer could
    /// reach outside the folder it belongs in.
    #[test]
    fn a_name_cannot_escape_its_folder() {
        assert_eq!(safe_file_name("../../.ssh/authorized_keys"), "authorized_keys");
        assert_eq!(safe_file_name("/etc/passwd"), "passwd");
        assert_eq!(safe_file_name("..\\..\\windows\\system32\\evil.dll"), "evil.dll");
        assert_eq!(safe_file_name(".."), "file");
        assert_eq!(safe_file_name("/"), "file");
        assert_eq!(safe_file_name(""), "file");
    }

    #[test]
    fn ordinary_names_are_left_alone() {
        assert_eq!(safe_file_name("report.pdf"), "report.pdf");
        assert_eq!(safe_file_name("holiday photo 2.jpg"), "holiday photo 2.jpg");
    }

    #[test]
    fn awkward_characters_are_replaced_rather_than_dropped() {
        assert_eq!(safe_file_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(safe_file_name("line\nbreak.txt"), "linebreak.txt");
    }

    #[test]
    fn very_long_names_are_cut_down() {
        let long = format!("{}.txt", "a".repeat(500));

        assert!(safe_file_name(&long).chars().count() <= 120);
    }

    #[test]
    fn a_folder_falls_back_to_the_peer_id_when_a_nickname_is_useless() {
        assert_eq!(folder_for("Alice", "12D3KooWabcdefghij"), "Alice");
        assert_eq!(folder_for("..", "12D3KooWabcdefghij"), "12D3KooWabcdefgh");
        assert_eq!(folder_for("", "12D3KooWabcdefghij"), "12D3KooWabcdefgh");
    }

    #[test]
    fn a_key_survives_being_written_down_and_read_back() {
        let key = FileKey::generate();
        let written = encode_key(&key);

        assert_eq!(written.len(), 64);
        assert_eq!(&decode_key(&written).unwrap(), key.as_bytes());
    }

    #[test]
    fn a_damaged_key_is_refused() {
        assert!(decode_key("nonsense").is_err());
        assert!(decode_key(&"z".repeat(64)).is_err());
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let directory = std::env::temp_dir().join(format!("feed-names-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();

        let first = available_path(&directory, "report.pdf");
        assert_eq!(first.file_name().unwrap(), "report.pdf");
        std::fs::write(&first, b"x").unwrap();

        let second = available_path(&directory, "report.pdf");
        assert_eq!(second.file_name().unwrap(), "report (2).pdf");

        let _ = std::fs::remove_dir_all(&directory);
    }
}
