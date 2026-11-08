//! Authenticated encryption for per-session UDP audio datagrams.
//!
//! The audio key is derived from the 256-bit random control-session token. That
//! token is delivered only over the certificate-pinned TLS control channel.
//! Packet headers remain visible for routing/reassembly, but are authenticated
//! as AEAD associated data. Payloads and PCM chunks are encrypted in place.

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;

use super::{FORMAT_OPUS, FORMAT_SILENCE, FORMAT_UNCOMPRESSED, pcm_datagram::PcmDatagram};

const SESSION_KEY_DOMAIN: &[u8] = b"gemacast-audio-session-key-v1";
pub(crate) const AUDIO_TAG_LEN: usize = 16;
const REPLAY_WINDOW_SEQUENCES: u64 = 480 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AudioSessionKey([u8; 32]);

impl AudioSessionKey {
    /// Derive a dedicated audio key from the random 32-byte hex session token.
    pub fn from_session_token(token: &str) -> Result<Self, String> {
        if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("audio session token must be 32 bytes encoded as 64 hexadecimal characters".into());
        }
        let mut hasher = Sha256::new();
        hasher.update(SESSION_KEY_DOMAIN);
        hasher.update(token.as_bytes());
        Ok(Self(hasher.finalize().into()))
    }

    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for AudioSessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AudioSessionKey([REDACTED])")
    }
}

pub(crate) struct AudioPacketCipher {
    key: LessSafeKey,
}

impl AudioPacketCipher {
    pub(crate) fn new(session_key: &AudioSessionKey) -> Self {
        let unbound = UnboundKey::new(&aead::CHACHA20_POLY1305, session_key.as_bytes())
            .expect("32-byte audio session key is valid for ChaCha20-Poly1305");
        Self { key: LessSafeKey::new(unbound) }
    }

    /// Encrypt only the payload and append the Poly1305 tag. The clear header
    /// determines the nonce and is authenticated as associated data.
    pub(crate) fn seal(&self, packet: &mut Vec<u8>) -> Result<(), String> {
        let header_len = packet_header_len(packet)?;
        let nonce_bytes = packet_nonce(packet, header_len)?;
        let (header, payload) = packet.split_at_mut(header_len);
        let tag = self.key
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(&header[..]),
                payload,
            )
            .map_err(|_| "audio packet encryption failed".to_string())?;
        packet.extend_from_slice(tag.as_ref());
        Ok(())
    }

    /// Decrypt a datagram in place and return the authenticated plaintext length.
    pub(crate) fn open(&self, packet: &mut [u8]) -> Result<usize, String> {
        let header_len = packet_header_len(packet)?;
        let nonce_bytes = packet_nonce(packet, header_len)?;
        if packet.len() < header_len + AUDIO_TAG_LEN {
            return Err("encrypted audio packet is shorter than its authentication tag".into());
        }
        let (header, ciphertext_and_tag) = packet.split_at_mut(header_len);
        let plaintext = self.key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(&header[..]),
                ciphertext_and_tag,
            )
            .map_err(|_| "audio packet authentication failed".to_string())?;
        Ok(header_len + plaintext.len())
    }
}

fn packet_header_len(packet: &[u8]) -> Result<usize, String> {
    if packet.len() < 9 {
        return Err("audio packet is shorter than its clear header".into());
    }
    match packet[8] {
        FORMAT_OPUS | FORMAT_UNCOMPRESSED | FORMAT_SILENCE => Ok(9),
        PcmDatagram::FORMAT if packet.len() >= PcmDatagram::HEADER => Ok(PcmDatagram::HEADER),
        PcmDatagram::FORMAT => Err("PCM chunk is shorter than its clear header".into()),
        _ => Err("audio packet has an unsupported clear-header format".into()),
    }
}

fn packet_nonce(packet: &[u8], header_len: usize) -> Result<[u8; 12], String> {
    let sequence: [u8; 8] = packet[..8]
        .try_into()
        .map_err(|_| "audio packet sequence number is missing".to_string())?;
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(&sequence);
    nonce[8] = packet[8];
    if header_len == PcmDatagram::HEADER {
        nonce[9] = packet[17];
    }
    // The final two bytes are reserved for future packet subtypes.
    Ok(nonce)
}

/// A small sliding replay window. It permits normal UDP reordering while
/// rejecting duplicate or stale authenticated datagrams.
#[derive(Default)]
pub(crate) struct AudioReplayWindow {
    highest_sequence: u64,
    initialized: bool,
    seen: HashSet<(u64, u8)>,
}

impl AudioReplayWindow {
    pub(crate) fn accept(&mut self, packet: &[u8]) -> bool {
        if packet.len() < 9 {
            return false;
        }
        let sequence = u64::from_be_bytes(packet[..8].try_into().unwrap());
        let subtype = if packet[8] == PcmDatagram::FORMAT {
            if packet.len() < PcmDatagram::HEADER {
                return false;
            }
            packet[17]
        } else {
            0
        };

        if self.initialized
            && self.highest_sequence.saturating_sub(sequence) > REPLAY_WINDOW_SEQUENCES
        {
            return false;
        }
        if !self.seen.insert((sequence, subtype)) {
            return false;
        }
        if !self.initialized || sequence > self.highest_sequence {
            self.highest_sequence = sequence;
            self.initialized = true;
            let floor = self.highest_sequence.saturating_sub(REPLAY_WINDOW_SEQUENCES);
            self.seen.retain(|(seen_sequence, _)| *seen_sequence >= floor);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> AudioSessionKey {
        AudioSessionKey::from_session_token(&"ab".repeat(32)).unwrap()
    }

    #[test]
    fn session_token_derivation_is_deterministic_and_redacted() {
        let first = key();
        let second = key();
        assert_eq!(first, second);
        assert_eq!(format!("{first:?}"), "AudioSessionKey([REDACTED])");
        assert!(AudioSessionKey::from_session_token("short").is_err());
    }

    #[test]
    fn authenticated_audio_packet_round_trips_and_rejects_tampering() {
        let cipher = AudioPacketCipher::new(&key());
        let mut packet = Vec::from([0, 0, 0, 0, 0, 0, 1, 2, FORMAT_OPUS, 1, 2, 3, 4]);
        let clear_len = packet.len();
        cipher.seal(&mut packet).unwrap();
        assert_eq!(packet.len(), clear_len + AUDIO_TAG_LEN);
        let opened_len = cipher.open(&mut packet).unwrap();
        assert_eq!(opened_len, clear_len);
        assert_eq!(&packet[..opened_len], &[0, 0, 0, 0, 0, 0, 1, 2, FORMAT_OPUS, 1, 2, 3, 4]);

        let mut tampered = vec![0, 0, 0, 0, 0, 0, 1, 3, FORMAT_OPUS, 1, 2, 3, 4];
        cipher.seal(&mut tampered).unwrap();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x80;
        assert!(cipher.open(&mut tampered).is_err());
    }

    #[test]
    fn pcm_chunk_nonce_includes_chunk_index_and_authenticates_its_header() {
        let cipher = AudioPacketCipher::new(&key());
        let mut first = vec![0u8; PcmDatagram::HEADER + PcmDatagram::PAYLOAD];
        first[..8].copy_from_slice(&42u64.to_be_bytes());
        first[8] = PcmDatagram::FORMAT;
        first[9..17].copy_from_slice(&7u64.to_be_bytes());
        first[17] = 0;
        let mut second = first.clone();
        second[17] = 1;
        cipher.seal(&mut first).unwrap();
        cipher.seal(&mut second).unwrap();
        assert!(cipher.open(&mut first).is_ok());
        assert!(cipher.open(&mut second).is_ok());
        second[17] = 0;
        assert!(cipher.open(&mut second).is_err());
    }

    #[test]
    fn replay_window_accepts_reordered_chunks_but_rejects_duplicates_and_stale_packets() {
        let mut window = AudioReplayWindow::default();
        let mut packet = vec![0u8; PcmDatagram::HEADER];
        packet[8] = PcmDatagram::FORMAT;
        packet[..8].copy_from_slice(&1_000_000u64.to_be_bytes());
        packet[17] = 1;
        assert!(window.accept(&packet));
        packet[17] = 0;
        assert!(window.accept(&packet));
        assert!(!window.accept(&packet));
        packet[17] = 1;
        packet[..8].copy_from_slice(&0u64.to_be_bytes());
        assert!(!window.accept(&packet));
    }
}
