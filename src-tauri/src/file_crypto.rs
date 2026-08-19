//! Encrypting a file for one recipient, a chunk at a time.
//!
//! Messages are sealed whole because they are small. A file cannot be, for three
//! reasons. It would have to be held in memory twice over, nothing could be
//! verified until the last byte arrived, and a transfer that broke half way
//! would have to start again.
//!
//! So a file is encrypted in chunks, each one sealed and authenticated by
//! itself. The receiver can verify and write each chunk as it lands, and a
//! transfer that dies can resume from the chunk it reached.
//!
//! # What holds the chunks together
//!
//! Sealing chunks separately raises the question of what stops somebody
//! reordering them, dropping the end off, or splicing in chunks from a different
//! file. Each of those is answered by what goes into the nonce:
//!
//! - The chunk's index is part of the nonce, so a chunk moved to a different
//!   position fails to open.
//! - The last chunk is marked as last, so a file cut short fails rather than
//!   looking complete.
//! - Every file has its own random key, so chunks cannot travel between files.
//!
//! This is the construction `age` uses, for the same reasons.
//!
//! # And what the tags do not cover
//!
//! The authentication tag on each chunk proves the bytes that arrived are the
//! bytes that were sealed. It says nothing about whether those were the right
//! bytes to begin with, which is a different question: a failing disk on the
//! sending side, or a file being written while it is read, would be encrypted
//! faithfully and arrive intact and wrong.
//!
//! That is what the whole-file hash is for. The sender hashes what it read, the
//! receiver hashes what it wrote, and the two are compared at the end.

use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use sha2::{Digest, Sha256};

/// How much of a file goes into one chunk, before encryption.
///
/// Each chunk costs 16 bytes of authentication tag, so smaller chunks mean more
/// overhead, and larger ones mean more memory in flight and coarser resumption.
/// At 64KB a file at the size ceiling is 160 chunks, and the overhead is well
/// under a tenth of a percent.
pub const CHUNK_SIZE: usize = 64 * 1024;

/// What one sealed chunk occupies.
pub const SEALED_CHUNK_SIZE: usize = CHUNK_SIZE + 16;

/// The largest file this node will send or accept **through a relay server**.
///
/// There is deliberately no limit on a file sent to somebody reachable directly.
/// Those bytes stay on the local network, cost nobody anything, and pass between
/// two people who have already added each other, which is the decision that
/// matters. A limit there would be a rule with nothing behind it.
///
/// A relayed transfer is different. The bytes cross a machine somebody else pays
/// for, and the person running it did not agree to any particular file. So this
/// one exists, and it is checked by both ends: the sender so it can say
/// something useful before sending, and the receiver so that the limit is not
/// merely a courtesy the sender may decline to observe.
pub const MAX_RELAYED_FILE_SIZE: u64 = 25 * 1024 * 1024;

/// The key for one file, used for that file and nothing else.
///
/// Travels to the recipient inside the sealed offer message, so it is protected
/// by the same encryption as everything else people say to each other, and never
/// passes anywhere a relay could see it.
pub struct FileKey([u8; 32]);

impl FileKey {
    /// A fresh key for a file about to be sent.
    pub fn generate() -> Self {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);

        Self(key)
    }

    /// Reads a key back from an offer.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new(Key::from_slice(&self.0))
    }
}

/// Builds the nonce for one chunk.
///
/// Eleven bytes of index and one byte saying whether this is the last chunk.
/// Both are what the chunk is bound to, so neither can be changed without the
/// chunk failing to open.
fn nonce_for(index: u64, is_last: bool) -> Nonce {
    let mut nonce = [0u8; 12];
    nonce[3..11].copy_from_slice(&index.to_be_bytes());
    nonce[11] = u8::from(is_last);

    *Nonce::from_slice(&nonce)
}

/// Seals one chunk of a file.
///
/// `is_last` must be true for the final chunk and false for every other one. It
/// is what makes a truncated file detectable rather than merely shorter.
pub fn seal_chunk(
    key: &FileKey,
    index: u64,
    is_last: bool,
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    if plaintext.len() > CHUNK_SIZE {
        return Err(format!(
            "a chunk may hold {} bytes and this one holds {}",
            CHUNK_SIZE,
            plaintext.len()
        ));
    }

    key.cipher()
        .encrypt(
            &nonce_for(index, is_last),
            Payload {
                msg: plaintext,
                aad: &[],
            },
        )
        .map_err(|_| "could not encrypt this part of the file".to_string())
}

/// Opens one chunk.
///
/// Fails if the chunk was altered, if it arrived in a different position from
/// the one it was sealed in, or if a chunk in the middle is being passed off as
/// the end of the file.
pub fn open_chunk(
    key: &FileKey,
    index: u64,
    is_last: bool,
    sealed: &[u8],
) -> Result<Vec<u8>, String> {
    key.cipher()
        .decrypt(
            &nonce_for(index, is_last),
            Payload {
                msg: sealed,
                aad: &[],
            },
        )
        .map_err(|_| format!("part {} of this file could not be read", index))
}

/// How many chunks a file of this size becomes.
///
/// An empty file is one empty chunk rather than none, so that every file has a
/// last chunk to mark and a transfer always has something to send.
pub fn chunk_count(file_size: u64) -> u64 {
    if file_size == 0 {
        return 1;
    }

    file_size.div_ceil(CHUNK_SIZE as u64)
}

/// Hashes a file as it is read or written.
///
/// Kept as a running total so neither side has to hold a whole file to check it,
/// and so the receiver can hash while writing rather than reading it all back
/// afterwards.
#[derive(Default)]
pub struct RunningHash(Sha256);

impl RunningHash {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The hash, as hex, which is what the offer carries and what the Files view
    /// can show.
    pub fn finish(self) -> String {
        self.0
            .finalize()
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    /// Hashes a run of bytes in one go.
    ///
    /// Only the tests want this. Everything real hashes a file as it streams
    /// past, which is what `RunningHash` is for, and having a whole-input
    /// version outside these tests would be a function nothing calls.
    fn hash_bytes(bytes: &[u8]) -> String {
        let mut hash = super::RunningHash::new();
        hash.update(bytes);

        hash.finish()
    }

    use super::*;

    /// Seals a whole file and returns its chunks, as a transfer would.
    fn seal_file(key: &FileKey, contents: &[u8]) -> Vec<Vec<u8>> {
        let chunks: Vec<&[u8]> = if contents.is_empty() {
            vec![&[]]
        } else {
            contents.chunks(CHUNK_SIZE).collect()
        };

        let last = chunks.len() - 1;

        chunks
            .iter()
            .enumerate()
            .map(|(index, chunk)| seal_chunk(key, index as u64, index == last, chunk).unwrap())
            .collect()
    }

    /// Opens chunks back into a file, as a receiver would.
    fn open_file(key: &FileKey, sealed: &[Vec<u8>]) -> Result<Vec<u8>, String> {
        let last = sealed.len() - 1;
        let mut contents = Vec::new();

        for (index, chunk) in sealed.iter().enumerate() {
            contents.extend(open_chunk(key, index as u64, index == last, chunk)?);
        }

        Ok(contents)
    }

    fn a_file(size: usize) -> Vec<u8> {
        (0..size).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn a_file_survives_the_round_trip() {
        let key = FileKey::generate();
        let contents = a_file(CHUNK_SIZE * 3 + 1234);

        let sealed = seal_file(&key, &contents);
        assert_eq!(sealed.len(), 4);
        assert_eq!(open_file(&key, &sealed).unwrap(), contents);
    }

    /// Every size that lands exactly on a chunk boundary is worth checking, since
    /// that is where an off-by-one lives.
    #[test]
    fn awkward_sizes_survive_too() {
        let key = FileKey::generate();

        for size in [0, 1, CHUNK_SIZE - 1, CHUNK_SIZE, CHUNK_SIZE + 1] {
            let contents = a_file(size);
            let sealed = seal_file(&key, &contents);

            assert_eq!(
                open_file(&key, &sealed).unwrap(),
                contents,
                "a file of {} bytes did not survive",
                size
            );
        }
    }

    #[test]
    fn the_wrong_key_reads_nothing() {
        let contents = a_file(5000);
        let sealed = seal_file(&FileKey::generate(), &contents);

        assert!(open_file(&FileKey::generate(), &sealed).is_err());
    }

    /// A chunk moved to a different position must fail, or a file could be
    /// rearranged by whoever carried it.
    #[test]
    fn chunks_cannot_be_reordered() {
        let key = FileKey::generate();
        let mut sealed = seal_file(&key, &a_file(CHUNK_SIZE * 3));

        sealed.swap(0, 1);

        assert!(open_file(&key, &sealed).is_err());
    }

    /// Cutting the end off a file must fail rather than producing a shorter
    /// file that looks complete. This is what marking the last chunk buys.
    #[test]
    fn a_file_cannot_be_cut_short() {
        let key = FileKey::generate();
        let sealed = seal_file(&key, &a_file(CHUNK_SIZE * 3));

        let truncated = &sealed[..2];

        assert!(
            open_file(&key, truncated).is_err(),
            "a truncated file must not open as if it were whole"
        );
    }

    /// And a chunk from the middle must not be passable as the end.
    #[test]
    fn a_middle_chunk_cannot_pose_as_the_end() {
        let key = FileKey::generate();
        let contents = a_file(CHUNK_SIZE * 2);
        let sealed = seal_file(&key, &contents);

        assert!(open_chunk(&key, 0, true, &sealed[0]).is_err());
    }

    #[test]
    fn altered_bytes_are_caught() {
        let key = FileKey::generate();
        let mut sealed = seal_file(&key, &a_file(5000));

        sealed[0][10] ^= 0x01;

        assert!(open_file(&key, &sealed).is_err());
    }

    /// Chunks from one file must be useless in another, which follows from every
    /// file having its own key.
    #[test]
    fn chunks_do_not_travel_between_files() {
        let (first, second) = (FileKey::generate(), FileKey::generate());
        let mut sealed = seal_file(&first, &a_file(CHUNK_SIZE * 2));
        let other = seal_file(&second, &a_file(CHUNK_SIZE * 2));

        sealed[0] = other[0].clone();

        assert!(open_file(&first, &sealed).is_err());
    }

    #[test]
    fn counts_chunks_including_for_an_empty_file() {
        assert_eq!(chunk_count(0), 1);
        assert_eq!(chunk_count(1), 1);
        assert_eq!(chunk_count(CHUNK_SIZE as u64), 1);
        assert_eq!(chunk_count(CHUNK_SIZE as u64 + 1), 2);
        assert_eq!(chunk_count(MAX_RELAYED_FILE_SIZE), 400);
    }

    /// The hash has to come out the same however the bytes were fed in, since one
    /// side hashes while reading and the other while writing.
    #[test]
    fn the_hash_does_not_depend_on_how_it_was_fed() {
        let contents = a_file(10_000);

        let mut piecemeal = RunningHash::new();
        for piece in contents.chunks(333) {
            piecemeal.update(piece);
        }

        assert_eq!(piecemeal.finish(), hash_bytes(&contents));
    }

    #[test]
    fn different_files_hash_differently() {
        assert_ne!(hash_bytes(b"the report"), hash_bytes(b"the report "));
    }
}
