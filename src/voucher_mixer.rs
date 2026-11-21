//! Desktop voucher selector 0x8a, ported from DeDRM's win_pure_math.py.

//! This is compatibility code for an observed algorithm. The reference
//! reproduces the arithmetic, but gives no design rationale for these
//! constants, rotations, or round counts.

const GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;
const MIX_MULTIPLIER: u64 = 0xbf58476d1ce4e5b9;

/// Produce the 256-byte HMAC key for the supported desktop voucher path.
/// The reference sorts its input pairs lexicographically by field name.
pub fn derive_vector(account: Option<&[u8]>, client: Option<&[u8]>, voucher: &[u8]) -> [u8; 256] {
    let mut pairs: Vec<(&[u8], &[u8])> = Vec::with_capacity(3);
    if let Some(value) = account {
        pairs.push((b"ACCOUNT_SECRET", value));
    }
    if let Some(value) = client {
        pairs.push((b"CLIENT_ID", value));
    }
    pairs.push((b"voucherId", voucher));

    // The source compares plain input field names to its encoded labels.
    // The plaintext labels therefore affect the pair mixer, while the initial
    // hash and mixer selection remain zero and three respectively.
    let mut bytes = [0u8; 256];
    let mut offset = 0;
    let mut seed = GOLDEN_RATIO;
    // Lay down the names XORed with cycling value bytes. Separately fold
    // every byte of each name and value into the seed for the remaining space.
    for (index, (name, value)) in pairs.into_iter().enumerate() {
        for (i, byte) in name.iter().enumerate() {
            let selected = if value.is_empty() {
                0xa5
            } else {
                value[i % value.len()]
            };
            bytes[offset] = selected ^ byte ^ 0xa5;
            offset += 1;
        }
        let name_hash = name.iter().fold(0x0123456789abcdef_u64, |acc, byte| {
            signed_byte(*byte).wrapping_mul(GOLDEN_RATIO) ^ (acc >> 27) ^ (acc << 5)
        });
        let value_hash = value.iter().fold(0xfedcba9876543210_u64, |acc, byte| {
            signed_byte(*byte).wrapping_mul(MIX_MULTIPLIER) ^ (acc >> 25) ^ (acc << 7)
        });
        let pair_number = index as u64 + 1;
        seed ^= name_hash.wrapping_mul(pair_number) ^ value_hash.wrapping_mul(pair_number + 1);
    }
    // Fill the rest of the 256-byte state with the reference's wrapping
    // integer recurrence and the current byte position.
    const SEED_STEP: u64 = 0x41c64e6d3b83917f;
    for (index, byte) in bytes.iter_mut().enumerate().skip(offset) {
        seed ^= seed >> 29;
        seed = seed.wrapping_mul(MIX_MULTIPLIER);
        seed ^= (seed >> 32) ^ (index as u64).wrapping_mul(SEED_STEP);
        *byte = ((index as u8) << 3) ^ seed as u8;
    }
    // The reference views the state as 32 little-endian 64-bit lanes.
    // The chosen branch applies the four-lane, two-lane, four-lane passes.
    let mut lanes: [u64; 32] = std::array::from_fn(|index| {
        let start = 8 * index;
        u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap())
    });
    mix_quarters_forward(&mut lanes);
    mix_pairs(&mut lanes);
    mix_quarters_reverse(&mut lanes);

    // XOR each pair with its counterpart in the other half of the state.
    // Carry two accumulators across the pairs and serialize each result as
    // little-endian words for the HMAC stage.
    let mut left = 1u64;
    let mut right = !left;
    for index in 0..16 {
        let a = lanes[2 * index] ^ lanes[(2 * index + 16) % 32];
        let b = lanes[2 * index + 1] ^ lanes[(2 * index + 17) % 32];
        let round = 1 + 16 * index as u64;
        let shift = (round & 15) as u32;
        left = (left.wrapping_add(a) ^ round).rotate_left(((round & 7) + 24) as u32)
            ^ (a >> (round & 31))
            ^ right;
        right = (right.wrapping_add(b) ^ (round >> 16))
            .rotate_left((((round >> 8) & 15) + 16) as u32)
            ^ (b << shift)
            ^ left;
        let start = 16 * index;
        bytes[start..start + 8]
            .copy_from_slice(&(left.rotate_left(32) ^ (b >> shift) ^ left).to_le_bytes());
        bytes[start + 8..start + 16]
            .copy_from_slice(&(right.rotate_left(16) ^ (a << shift) ^ right).to_le_bytes());
    }
    bytes
}

// Match the reference's signed byte loads, including sign extension before
// multiplication and reduction modulo 2^64.
fn signed_byte(byte: u8) -> u64 {
    byte as i8 as i64 as u64
}

// Rearrange nibbles within each lane, then apply the reference's four
// successive shift-XOR steps, written here as their equivalent single XOR.
fn nibble_pass(lanes: &mut [u64; 32]) {
    for lane in lanes {
        let swapped = ((*lane & 0xff0f0f0f0f0f0f0f) << 4) | ((*lane >> 4) & 0x0f0f0f0f0f0f0f0f);
        *lane = swapped ^ (swapped << 1) ^ (swapped << 2) ^ (swapped << 3) ^ (swapped << 4);
    }
}

// Three rounds exchange shifted bits between adjacent lanes, then apply
// the nibble pass. A position-dependent final step transforms each lane.
fn mix_pairs(lanes: &mut [u64; 32]) {
    for round in 0..3u32 {
        let step = u64::from(round.wrapping_mul(0x9e3779b9) ^ 1);
        for (index, pair) in lanes.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let a = pair[0];
            let b = pair[1];
            pair[0] = ((2 * index + 1) as u64 * step) ^ (b >> 23) ^ (a << 17);
            pair[1] = ((2 * index + 2) as u64 * step)
                ^ (b << 23)
                ^ (a >> 17)
                ^ 0u64.wrapping_sub(MIX_MULTIPLIER);
        }
        nibble_pass(lanes);
    }
    for (index, lane) in lanes.iter_mut().enumerate() {
        let mixed = lane.wrapping_mul(0x1b) ^ (*lane >> 8) ^ (index as u64 * 0x41c64e6d) ^ 1;
        *lane = (mixed >> 19) ^ (mixed << 13);
    }
}

// Three passes over four neighboring lanes. Each update combines the
// previous lanes using wrapping addition, rotations and XOR.
fn mix_quarters_forward(lanes: &mut [u64; 32]) {
    for round in 0..3 {
        let seed = (1u64 << (32 - round)) ^ (1u64 >> round);
        let second = seed >> 8;
        let third = seed >> 16;
        for (index, lanes) in lanes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let a = lanes[0];
            let d = lanes[3];
            let b = (a.wrapping_add(d) ^ seed).rotate_left(((seed & 7) + 7) as u32) ^ lanes[1];
            let c = (b.wrapping_add(a) ^ third).rotate_left(((second & 7) + 9) as u32) ^ lanes[2];
            let d = d ^ (c.wrapping_add(b) ^ (seed << 8)).rotate_left(((third & 7) + 13) as u32);
            let step = (round + 4 * index + 1) as u64;
            let a = a
                ^ (d.wrapping_add(c) ^ second).rotate_left((((seed >> 24) & 7) + 18) as u32)
                ^ (c >> 17)
                ^ step;
            let b = b ^ (step >> 16) ^ (d << 23);
            lanes.copy_from_slice(&[
                a,
                b,
                (step << 8) ^ (a >> 29) ^ c,
                (step >> 24) ^ (b << 31) ^ d,
            ]);
        }
    }
}

// The final four-lane pass uses a different update order and five rounds.
// Run the nibble transform after each round across the whole state.
fn mix_quarters_reverse(lanes: &mut [u64; 32]) {
    for round in 0..5 {
        let seed = 1u64 << round;
        let third = seed >> 16;
        for (index, lanes) in lanes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let step = (4 * index + 1) as u64;
            let a = lanes[0].wrapping_add(lanes[1]) ^ seed;
            let d = (a ^ lanes[3]).rotate_left(((seed & 7) + 16) as u32);
            let c = lanes[2].wrapping_add(d) ^ third;
            let b = (c ^ lanes[1]).rotate_left((((seed >> 8) & 7) + 12) as u32);
            let a = a.wrapping_add(b) ^ step;
            let d = (d ^ a).rotate_left(((third & 7) + 8) as u32);
            let c = c.wrapping_add(d) ^ (step >> 16);
            lanes.copy_from_slice(&[
                a,
                (c ^ b).rotate_left((((seed >> 24) & 7) + 4) as u32),
                c,
                d,
            ]);
        }
        nibble_pass(lanes);
    }
}

#[cfg(test)]
mod tests {
    use super::derive_vector;
    use sha2::{Digest, Sha256};

    #[test]
    fn matches_reference_mixer_for_synthetic_inputs() {
        let vector = derive_vector(Some(b"abc"), Some(b"def"), b"voucher");
        assert_eq!(
            <[u8; 32]>::from(Sha256::digest(vector)),
            [
                0xae, 0xa5, 0x68, 0x47, 0x17, 0xf6, 0xe6, 0x5e, 0x2b, 0xae, 0xf3, 0xb6, 0x82, 0x22,
                0xcf, 0x40, 0x5c, 0x5f, 0xe0, 0x1e, 0x7b, 0x7b, 0x6d, 0x30, 0x26, 0x2c, 0x33, 0xc8,
                0x23, 0x7d, 0xc5, 0x04,
            ]
        );
    }
}
