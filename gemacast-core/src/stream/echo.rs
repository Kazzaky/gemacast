//! Authenticated UDP echo pings for measuring raw wire RTT.
//!
//! Echo packets use a key derived with a separate domain from the audio key.
//! Their sequence numbers therefore occupy an independent nonce space, and
//! the timestamp stays encrypted. The PC verifies a ping before reflecting the
//! exact datagram; the phone verifies the reflected datagram before reporting RTT.

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Instant;

use crate::audio::AudioSessionKey;

const MAGIC: [u8; 2] = [0xF1, 0x1E];
const ECHO_KEY_DOMAIN: &[u8] = b"gemacast-udp-echo-key-v1";
const HEADER_LEN: usize = 10; // magic + sequence
const TIMESTAMP_LEN: usize = 8;
const TAG_LEN: usize = 16;
pub(crate) const ECHO_PACKET_LEN: usize = HEADER_LEN + TIMESTAMP_LEN + TAG_LEN;
const REPLAY_WINDOW: u64 = 1024;

static PING_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub(crate) struct EchoPacketCipher {
    key: LessSafeKey,
    next_sequence: AtomicU64,
}

impl EchoPacketCipher {
    pub(crate) fn new(session_key: &AudioSessionKey) -> Self {
        let derived_key = session_key.derive_subkey(ECHO_KEY_DOMAIN);
        let unbound = UnboundKey::new(&aead::CHACHA20_POLY1305, &derived_key)
            .expect("32-byte echo key is valid for ChaCha20-Poly1305");
        Self {
            key: LessSafeKey::new(unbound),
            next_sequence: AtomicU64::new(Self::now_micros()),
        }
    }

    /// Build a fresh authenticated ping. A process-wide monotonic timestamp
    /// seeds the counter and the counter allocates each nonce exactly once.
    pub(crate) fn build(&self) -> Result<Vec<u8>, String> {
        let sequence = self
            .next_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| "echo packet sequence space exhausted".to_string())?;

        let mut packet = Vec::with_capacity(ECHO_PACKET_LEN);
        packet.extend_from_slice(&MAGIC);
        packet.extend_from_slice(&sequence.to_be_bytes());
        let header = packet.as_slice();

        packet.extend_from_slice(&Self::now_micros().to_be_bytes());
        let nonce = Self::nonce(sequence);
        let tag = self
            .key
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(header),
                &mut packet[HEADER_LEN..],
            )
            .map_err(|_| "echo packet encryption failed".to_string())?;
        packet.extend_from_slice(tag.as_ref());
        debug_assert_eq!(packet.len(), ECHO_PACKET_LEN);
        Ok(packet)
    }

    /// Authenticate and decrypt a ping, returning its nonce sequence and
    /// process-monotonic send timestamp. The caller owns replay-window policy.
    pub(crate) fn open(&self, packet: &[u8]) -> Result<(u64, u64), String> {
        if !Self::matches(packet, packet.len()) {
            return Err("invalid echo packet header or length".into());
        }

        let sequence = u64::from_be_bytes(
            packet[2..HEADER_LEN]
                .try_into()
                .map_err(|_| "echo packet sequence is missing")?,
        );
        let mut ciphertext_and_tag = packet[HEADER_LEN..].to_vec();
        let plaintext = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(Self::nonce(sequence)),
                Aad::from(&packet[..HEADER_LEN]),
                &mut ciphertext_and_tag,
            )
            .map_err(|_| "echo packet authentication failed")?;
        if plaintext.len() != TIMESTAMP_LEN {
            return Err("echo packet timestamp has an invalid length".into());
        }
        let sent_micros = u64::from_be_bytes(
            plaintext
                .try_into()
                .map_err(|_| "echo packet timestamp is missing")?,
        );
        Ok((sequence, sent_micros))
    }

    pub(crate) fn matches(packet: &[u8], len: usize) -> bool {
        len == ECHO_PACKET_LEN
            && packet.len() >= ECHO_PACKET_LEN
            && packet[..2] == MAGIC
    }

    pub(crate) fn round_trip_ms(sent_micros: u64) -> f32 {
        Self::now_micros().saturating_sub(sent_micros) as f32 / 1000.0
    }

    fn nonce(sequence: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&sequence.to_be_bytes());
        nonce[8..10].copy_from_slice(&MAGIC);
        nonce
    }

    fn now_micros() -> u64 {
        PING_EPOCH.elapsed().as_micros().min(u64::MAX as u128) as u64
    }
}

/// Replay protection for authenticated echo pings. UDP reordering is tolerated
/// within a small window while duplicates and stale packets are discarded.
#[derive(Default)]
pub(crate) struct EchoReplayWindow {
    highest_sequence: u64,
    initialized: bool,
    seen: HashSet<u64>,
}

impl EchoReplayWindow {
    pub(crate) fn accept(&mut self, sequence: u64) -> bool {
        if self.initialized
            && self.highest_sequence.saturating_sub(sequence) > REPLAY_WINDOW
        {
            return false;
        }
        if !self.seen.insert(sequence) {
            return false;
        }
        if !self.initialized || sequence > self.highest_sequence {
            self.highest_sequence = sequence;
            self.initialized = true;
            let floor = self.highest_sequence.saturating_sub(REPLAY_WINDOW);
            self.seen.retain(|seen| *seen >= floor);
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
    fn echo_ping_round_trips_through_an_independent_cipher() {
        let sender = EchoPacketCipher::new(&key());
        let receiver = EchoPacketCipher::new(&key());
        let packet = sender.build().unwrap();

        assert!(EchoPacketCipher::matches(&packet, packet.len()));
        assert_eq!(packet.len(), ECHO_PACKET_LEN);
        let (_sequence, sent_micros) = receiver.open(&packet).unwrap();
        let rtt = EchoPacketCipher::round_trip_ms(sent_micros);
        assert!((0.0..1_000.0).contains(&rtt), "unexpected RTT {rtt}");
    }

    #[test]
    fn echo_ping_authentication_rejects_tampering_and_wrong_keys() {
        let sender = EchoPacketCipher::new(&key());
        let wrong_key = AudioSessionKey::from_session_token(&"cd".repeat(32)).unwrap();
        let receiver = EchoPacketCipher::new(&wrong_key);
        let packet = sender.build().unwrap();

        assert!(receiver.open(&packet).is_err());

        let mut tampered = packet.clone();
        tampered[HEADER_LEN] ^= 0x40;
        assert!(EchoPacketCipher::new(&key()).open(&tampered).is_err());
    }

    #[test]
    fn echo_ping_replay_window_accepts_reordering_and_rejects_duplicates() {
        let mut replay = EchoReplayWindow::default();
        assert!(replay.accept(100));
        assert!(replay.accept(102));
        assert!(replay.accept(101));
        assert!(!replay.accept(101));
        assert!(!replay.accept(1));
    }

    #[test]
    fn echo_ping_does_not_accept_truncated_or_non_echo_packets() {
        let cipher = EchoPacketCipher::new(&key());
        let packet = cipher.build().unwrap();
        assert!(!EchoPacketCipher::matches(&packet, ECHO_PACKET_LEN - 1));
        assert!(!EchoPacketCipher::matches(&[0u8; ECHO_PACKET_LEN], ECHO_PACKET_LEN));
    }

    #[test]
    fn echo_nonce_sequences_are_unique_for_each_built_ping() {
        let cipher = EchoPacketCipher::new(&key());
        let first = cipher.build().unwrap();
        let second = cipher.build().unwrap();
        let (first_sequence, _) = EchoPacketCipher::new(&key()).open(&first).unwrap();
        let (second_sequence, _) = EchoPacketCipher::new(&key()).open(&second).unwrap();
        assert_ne!(first_sequence, second_sequence);
    }
}
