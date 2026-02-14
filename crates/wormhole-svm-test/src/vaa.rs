//! VAA construction utilities for testing.

use sha3::{Digest, Keccak256};

use crate::TestGuardianSet;

/// Specifies whether a VAA operation should be replay-protected.
///
/// Used by [`VaaChecks`] to control the automatic replay test in `with_vaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReplayProtection {
    /// The operation can be replayed (no replay protection check).
    /// Use this for operations that are intentionally idempotent or for
    /// testing error paths.
    Replayable,

    /// The operation must NOT be replayable (default).
    /// After successful execution, `with_vaa` will attempt to replay the
    /// same VAA. If the replay succeeds, the test fails with
    /// `ReplayProtectionMissing`.
    #[default]
    NonReplayable,
}

/// Controls which automatic negative tests `with_vaa` runs.
///
/// By default all checks are enabled. Disable specific checks for instructions
/// where a field is intentionally unchecked (e.g. `initialize` derives its PDA
/// from the emitter address, so any address is valid).
#[derive(Clone, Copy)]
pub struct VaaChecks {
    /// Test that the program rejects a VAA with a different emitter chain.
    pub emitter_chain: bool,
    /// Test that the program rejects a VAA with a different emitter address.
    pub emitter_address: bool,
    /// Test that the program rejects a replayed VAA.
    pub replay: ReplayProtection,
}

impl Default for VaaChecks {
    fn default() -> Self {
        Self {
            emitter_chain: true,
            emitter_address: true,
            replay: ReplayProtection::default(),
        }
    }
}

/// A test VAA for construction and signing.
#[derive(Clone)]
pub struct TestVaa {
    /// The emitter chain ID.
    pub emitter_chain: u16,
    /// The emitter address (32 bytes).
    pub emitter_address: [u8; 32],
    /// The sequence number.
    pub sequence: u64,
    /// The payload bytes.
    pub payload: Vec<u8>,
    /// The timestamp (defaults to 1234567890).
    pub timestamp: u32,
    /// The nonce (defaults to 0).
    pub nonce: u32,
    /// The consistency level (defaults to 1 = Confirmed).
    pub consistency_level: u8,
    /// The guardian set index (defaults to 0).
    pub guardian_set_index: u32,
    /// Which automatic negative tests to run in `with_vaa`.
    pub checks: VaaChecks,
}

impl TestVaa {
    /// Create a new test VAA with required fields and sensible defaults.
    pub fn new(
        emitter_chain: u16,
        emitter_address: [u8; 32],
        sequence: u64,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            emitter_chain,
            emitter_address,
            sequence,
            payload,
            timestamp: 1234567890,
            nonce: 0,
            consistency_level: 1,
            guardian_set_index: 0,
            checks: VaaChecks::default(),
        }
    }

    /// Build the VAA body bytes (without version, guardian set index, or signatures).
    pub fn body(&self) -> Vec<u8> {
        let mut body = Vec::new();

        // Timestamp (4 bytes, big-endian)
        body.extend_from_slice(&self.timestamp.to_be_bytes());

        // Nonce (4 bytes, big-endian)
        body.extend_from_slice(&self.nonce.to_be_bytes());

        // Emitter chain (2 bytes, big-endian)
        body.extend_from_slice(&self.emitter_chain.to_be_bytes());

        // Emitter address (32 bytes)
        body.extend_from_slice(&self.emitter_address);

        // Sequence (8 bytes, big-endian)
        body.extend_from_slice(&self.sequence.to_be_bytes());

        // Consistency level (1 byte)
        body.push(self.consistency_level);

        // Payload
        body.extend_from_slice(&self.payload);

        body
    }

    /// Compute the VAA digest (double keccak256 of body).
    pub fn digest(&self) -> [u8; 32] {
        let body = self.body();
        let message_hash = Keccak256::digest(&body);
        Keccak256::digest(message_hash).into()
    }

    /// Build a signed VAA with all guardians in the set.
    pub fn sign(&self, guardians: &TestGuardianSet) -> Vec<u8> {
        let body = self.body();
        let signatures = guardians.sign_vaa_body(&body);
        self.build_signed_vaa(&body, &signatures)
    }

    /// Build a signed VAA with specific guardians (by index).
    pub fn sign_with(&self, guardians: &TestGuardianSet, indices: &[u8]) -> Vec<u8> {
        let body = self.body();
        let signatures = guardians.sign_vaa_body_with(&body, indices);
        self.build_signed_vaa(&body, &signatures)
    }

    /// Get guardian signatures for use with post_signatures instruction.
    pub fn guardian_signatures(&self, guardians: &TestGuardianSet) -> Vec<[u8; 66]> {
        let body = self.body();
        guardians.sign_vaa_body(&body)
    }

    /// Build the full signed VAA bytes.
    fn build_signed_vaa(&self, body: &[u8], signatures: &[[u8; 66]]) -> Vec<u8> {
        let mut vaa = Vec::new();

        // Version (1 byte)
        vaa.push(1);

        // Guardian set index (4 bytes, big-endian)
        vaa.extend_from_slice(&self.guardian_set_index.to_be_bytes());

        // Number of signatures (1 byte)
        vaa.push(signatures.len() as u8);

        // Signatures (66 bytes each)
        for sig in signatures {
            vaa.extend_from_slice(sig);
        }

        // Body
        vaa.extend_from_slice(body);

        vaa
    }
}

/// Helper to create an emitter address from a 20-byte address (right-aligned).
///
/// Useful for EVM-style addresses that are 20 bytes.
pub fn emitter_address_from_20(addr: [u8; 20]) -> [u8; 32] {
    let mut result = [0u8; 32];
    result[12..32].copy_from_slice(&addr);
    result
}

/// Helper to create an emitter address from a Pubkey-like 32-byte value.
pub fn emitter_address_from_32(addr: [u8; 32]) -> [u8; 32] {
    addr
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestGuardian;

    #[test]
    fn test_vaa_body_structure() {
        let vaa = TestVaa::new(
            1, // Solana
            [0xAB; 32],
            42,
            vec![1, 2, 3, 4],
        );

        let body = vaa.body();

        // Timestamp (4) + Nonce (4) + Chain (2) + Emitter (32) + Seq (8) + Consistency (1) + Payload (4)
        assert_eq!(body.len(), 4 + 4 + 2 + 32 + 8 + 1 + 4);

        // Check timestamp
        let ts = u32::from_be_bytes(body[0..4].try_into().unwrap());
        assert_eq!(ts, 1234567890);

        // Check chain
        let chain = u16::from_be_bytes(body[8..10].try_into().unwrap());
        assert_eq!(chain, 1);

        // Check sequence
        let seq = u64::from_be_bytes(body[42..50].try_into().unwrap());
        assert_eq!(seq, 42);
    }

    #[test]
    fn test_signed_vaa_structure() {
        let guardians = TestGuardianSet::single(TestGuardian::default());
        let vaa = TestVaa::new(1, [0xAB; 32], 42, vec![1, 2, 3, 4]);

        let signed = vaa.sign(&guardians);

        // Version (1) + GS Index (4) + Num Sigs (1) + Sig (66) + Body (55)
        assert_eq!(signed.len(), 1 + 4 + 1 + 66 + 55);

        // Check version
        assert_eq!(signed[0], 1);

        // Check guardian set index
        let gs_index = u32::from_be_bytes(signed[1..5].try_into().unwrap());
        assert_eq!(gs_index, 0);

        // Check num signatures
        assert_eq!(signed[5], 1);
    }

    #[test]
    fn test_multi_guardian_signing() {
        let guardians = TestGuardianSet::generate(3, 123);
        let vaa = TestVaa::new(1, [0xAB; 32], 42, vec![]);

        let signed = vaa.sign(&guardians);

        // Check num signatures
        assert_eq!(signed[5], 3);

        // Check signature indices
        assert_eq!(signed[6], 0); // First sig, index 0
        assert_eq!(signed[6 + 66], 1); // Second sig, index 1
        assert_eq!(signed[6 + 132], 2); // Third sig, index 2
    }

    #[test]
    fn test_sign_with_subset() {
        let guardians = TestGuardianSet::generate(5, 456);
        let vaa = TestVaa::new(1, [0xAB; 32], 42, vec![]);

        let signed = vaa.sign_with(&guardians, &[1, 3]);

        // Check num signatures
        assert_eq!(signed[5], 2);

        // Check signature indices
        assert_eq!(signed[6], 1); // First sig from guardian 1
        assert_eq!(signed[6 + 66], 3); // Second sig from guardian 3
    }

    #[test]
    fn test_emitter_address_helpers() {
        let addr20 = [0xAB; 20];
        let result = emitter_address_from_20(addr20);

        assert_eq!(&result[0..12], &[0u8; 12]);
        assert_eq!(&result[12..32], &addr20);
    }
}
