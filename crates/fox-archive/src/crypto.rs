//! The two obfuscation layers used by QAR entries.
//!
//! * Layer 1 ("decrypt1") is applied to every entry: a position-dependent XOR
//!   keyed by the low half of the entry hash (and, in QAR version 2, by the
//!   entry's stored MD5).
//! * Layer 2 ("decrypt2") is an LCG keystream applied to entries that start
//!   with one of the [`MAGIC_SHORT`]/[`MAGIC_LONG`] headers.

/// Layer-2 header with an 8-byte header (`magic`, `key`).
pub const MAGIC_SHORT: u32 = 0xA0F8_EFE6;
/// Layer-2 header with a 16-byte header (`magic`, `key`, `size`, `size`).
pub const MAGIC_LONG: u32 = 0xE3F8_EFE6;

pub fn layer2_header_size(magic: u32) -> usize {
    match magic {
        MAGIC_SHORT => 8,
        MAGIC_LONG => 16,
        _ => 0,
    }
}

const LAYER1_TABLE: [u32; 8] = [
    0xBB8A_DEDB,
    0x6522_9958,
    0x0845_3206,
    0x8812_1302,
    0x4C34_4955,
    0x2C02_F10C,
    0x4887_F823,
    0xF381_8583,
];

/// Parameters for layer 1 of one entry.
#[derive(Clone, Copy, Debug)]
pub struct Layer1 {
    pub version: u32,
    pub hash_low: u32,
    /// Only used by version 2: one half of the entry MD5, picked by `hash_low & 1`.
    pub seed: u64,
}

impl Layer1 {
    pub fn new(version: u32, hash: u64, md5: &[u8; 16]) -> Self {
        let hash_low = hash as u32;
        let half = (hash_low % 2) as usize * 8;
        let seed = u64::from_le_bytes(md5[half..half + 8].try_into().unwrap());
        Self {
            version,
            hash_low,
            seed,
        }
    }

    /// 64-bit XOR mask for the 8-byte block starting `block_offset` bytes into the entry.
    #[inline]
    fn mask(&self, block_offset: u64) -> u64 {
        let index = if self.version != 2 {
            2 * ((self.hash_low as u64 + block_offset / 11) % 4)
        } else {
            2 * ((self.hash_low as u64)
                .wrapping_add(self.seed)
                .wrapping_add(block_offset / 11)
                % 4)
        } as usize;
        let mut m = (LAYER1_TABLE[index] as u64) | ((LAYER1_TABLE[index + 1] as u64) << 32);
        if self.version == 2 {
            m ^= self.seed;
        }
        m
    }

    /// Decrypt (or encrypt; the operation is an involution) `data`, which
    /// starts `start` bytes into the entry. `start` must be 8-aligned.
    pub fn apply(&self, data: &mut [u8], start: u64) {
        debug_assert_eq!(start % 8, 0);
        let mut chunks = data.chunks_exact_mut(8);
        let mut off = start;
        for block in &mut chunks {
            let v = u64::from_le_bytes((&*block).try_into().unwrap()) ^ self.mask(off);
            block.copy_from_slice(&v.to_le_bytes());
            off += 8;
        }
        let tail = chunks.into_remainder();
        let m = self.mask(off).to_le_bytes();
        for (b, k) in tail.iter_mut().zip(m) {
            *b ^= k;
        }
    }
}

/// Layer 2 keystream. Trailing bytes that do not fill a 32-bit word are left as is.
pub fn layer2_apply(data: &mut [u8], key: u32) {
    let mul_key = key.wrapping_mul(278);
    let mut block_key = key | ((key ^ 25974) << 16);
    for word in data.chunks_exact_mut(4) {
        let v = u32::from_le_bytes((&*word).try_into().unwrap()) ^ block_key;
        word.copy_from_slice(&v.to_le_bytes());
        block_key = mul_key.wrapping_add(block_key.wrapping_mul(48_828_125));
    }
}

/// Decrypt a Lua script stored inside an FPK. Encrypted scripts start with
/// `0x1B` or `0x1C`; the key is `!StrCode64(lowercase file name)`. Returns
/// `None` if the result does not end with the expected NUL terminator.
pub fn decrypt_fpk_script(data: &[u8], file_name: &str) -> Option<Vec<u8>> {
    if data.len() < 2 || !(data[0] == 0x1B || data[0] == 0x1C) {
        return None;
    }
    let lower = file_name.to_ascii_lowercase();
    let mut key = (!fox_hash::str_code64(lower.as_bytes())).to_le_bytes();
    let mut out = Vec::with_capacity(data.len() - 1);
    for (i, &b) in data[1..].iter().enumerate() {
        key[i % 8] ^= b;
        out.push(key[i % 8]);
    }
    if out.last() != Some(&0) {
        return None;
    }
    out.pop();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer1_is_involution_and_chunk_consistent() {
        let l = Layer1::new(1, 0x1234_5678_9abc_def0, &[7; 16]);
        let orig: Vec<u8> = (0..1000u32).map(|i| (i * 31) as u8).collect();
        let mut whole = orig.clone();
        l.apply(&mut whole, 0);
        let mut split = orig.clone();
        let (a, b) = split.split_at_mut(512);
        l.apply(a, 0);
        l.apply(b, 512);
        assert_eq!(whole, split);
        l.apply(&mut whole, 0);
        assert_eq!(whole, orig);
    }
}
