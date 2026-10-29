//! One cryptographic digest, implemented here rather than imported.
//!
//! A stored experiment has to be able to say "this is the model that produced these numbers", and
//! the cheapest way to be wrong about that is a checksum with no collision resistance. SHA-256 is
//! about ninety lines, has published test vectors that make a real correctness test, and is not one
//! of APORIA's algorithms — it is the boring infrastructure the dependency policy leaves to `std`
//! and to hand-written code rather than to a crate.
//!
//! Nothing here is a security claim. It detects a model file that changed underneath an experiment,
//! including by accident, which is the failure mode a research archive actually has.

use std::fmt::Write as _;

#[expect(
    clippy::unreadable_literal,
    reason = "SHA-256 round constants transcribed from FIPS 180-4; grouping their digits would make a transcription error harder to spot"
)]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 of a byte string, as 64 lowercase hex characters.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// SHA-256 of a byte string.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finish()
}

/// The same SHA-256, fed in pieces.
///
/// The sandbox hashes a child process's whole output stream while retaining only a prefix of it.
/// A one-shot function would make the recorded digest depend on the display cap, which is exactly
/// the kind of measurement that must not move when a limit moves.
#[derive(Clone, Debug)]
pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    #[expect(
        clippy::unreadable_literal,
        reason = "the initial state vector is transcribed from FIPS 180-4, same as the round constants"
    )]
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0u8; 64],
            filled: 0,
            len: 0,
        }
    }

    /// Feed bytes. Any chunking produces the same digest as one call with the concatenation.
    pub fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        if self.filled > 0 {
            let take = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                compress(&mut self.state, &self.block);
                self.filled = 0;
            }
        }
        while data.len() >= 64 {
            let mut whole = [0u8; 64];
            whole.copy_from_slice(&data[..64]);
            compress(&mut self.state, &whole);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.block[..data.len()].copy_from_slice(data);
            self.filled = data.len();
        }
    }

    /// Pad with `0x80`, zeros, and the bit length; return the digest.
    #[must_use]
    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.len.wrapping_mul(8);
        self.block[self.filled] = 0x80;
        self.block[self.filled + 1..].fill(0);
        if self.filled >= 56 {
            compress(&mut self.state, &self.block);
            self.block = [0u8; 64];
        }
        self.block[56..].copy_from_slice(&bits.to_be_bytes());
        compress(&mut self.state, &self.block);

        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

fn compress(h: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    let updates = [a, b, c, d, e, f, g, hh];
    for (target, add) in h.iter_mut().zip(updates) {
        *target = target.wrapping_add(add);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_vectors_match() {
        // FIPS 180-4 examples, which is what makes this a test and not a restatement.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn a_long_input_crosses_the_padding_boundary() {
        // The 1000-byte digest below was measured with `certutil -hashfile` on this machine rather
        // than recalled, and the 1e6 'a' vector from the standard is not used because it is slow to
        // generate in a unit test.
        // Inputs at 55..64 bytes need the extra block, the case most likely to be wrong.
        for len in 55..=64 {
            let data = vec![b'a'; len];
            assert_eq!(sha256(&data).len(), 32, "length {len}");
        }
        assert_eq!(
            sha256_hex(&vec![b'a'; 1000]),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    #[test]
    fn different_inputs_give_different_digests() {
        assert_ne!(sha256_hex(b"model x { }"), sha256_hex(b"model y { }"));
    }

    #[test]
    fn streaming_and_one_shot_agree_across_block_boundaries() {
        // The sandbox hashes a whole child-process stream while retaining at most a prefix of it.
        // If the streaming form disagreed with the one-shot form, the recorded digest would depend
        // on the output cap -- a measurement that changes when a display limit changes.
        for len in [0usize, 1, 55, 56, 63, 64, 65, 119, 120, 128, 1000] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let mut streamed = Sha256::new();
            for chunk in data.chunks(7) {
                streamed.update(chunk);
            }
            assert_eq!(streamed.finish(), sha256(&data), "length {len}");
        }
        assert_eq!(Sha256::new().finish(), sha256(&[]));
    }
}
