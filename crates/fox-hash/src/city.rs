//! CityHash64, version 1.0.x (the 2011 release the Fox Engine was built against).
//!
//! CityHash 1.1 changed `HashLen0to16` and `HashLen33to64`, so the `cityhash`
//! crates on crates.io that track 1.1 produce different values. This is a direct
//! port of the 1.0.3 reference algorithm (MIT, Google).

const K0: u64 = 0xc3a5_c85c_97cb_3127;
const K1: u64 = 0xb492_b66f_be98_f273;
const K2: u64 = 0x9ae1_6a3b_2f90_404f;
const K3: u64 = 0xc949_d7c7_509e_6557;

#[inline]
fn fetch64(s: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(s[at..at + 8].try_into().unwrap())
}

#[inline]
fn fetch32(s: &[u8], at: usize) -> u64 {
    u32::from_le_bytes(s[at..at + 4].try_into().unwrap()) as u64
}

#[inline]
fn rotate(v: u64, shift: u32) -> u64 {
    if shift == 0 { v } else { v.rotate_right(shift) }
}

#[inline]
fn shift_mix(v: u64) -> u64 {
    v ^ (v >> 47)
}

#[inline]
pub fn hash128_to_64(u: u64, v: u64) -> u64 {
    const MUL: u64 = 0x9ddf_ea08_eb38_2d69;
    let mut a = (u ^ v).wrapping_mul(MUL);
    a ^= a >> 47;
    let mut b = (v ^ a).wrapping_mul(MUL);
    b ^= b >> 47;
    b.wrapping_mul(MUL)
}

#[inline]
fn hash_len16(u: u64, v: u64) -> u64 {
    hash128_to_64(u, v)
}

fn hash_len_0_to_16(s: &[u8]) -> u64 {
    let len = s.len();
    if len > 8 {
        let a = fetch64(s, 0);
        let b = fetch64(s, len - 8);
        // RotateByAtLeast1: shift is always in 9..=16 here.
        return hash_len16(a, b.wrapping_add(len as u64).rotate_right(len as u32)) ^ b;
    }
    if len >= 4 {
        let a = fetch32(s, 0);
        return hash_len16((len as u64).wrapping_add(a << 3), fetch32(s, len - 4));
    }
    if len > 0 {
        let a = s[0] as u64;
        let b = s[len >> 1] as u64;
        let c = s[len - 1] as u64;
        let y = a.wrapping_add(b << 8);
        let z = (len as u64).wrapping_add(c << 2);
        return shift_mix(y.wrapping_mul(K2) ^ z.wrapping_mul(K3)).wrapping_mul(K2);
    }
    K2
}

fn hash_len_17_to_32(s: &[u8]) -> u64 {
    let len = s.len();
    let a = fetch64(s, 0).wrapping_mul(K1);
    let b = fetch64(s, 8);
    let c = fetch64(s, len - 8).wrapping_mul(K2);
    let d = fetch64(s, len - 16).wrapping_mul(K0);
    hash_len16(
        rotate(a.wrapping_sub(b), 43)
            .wrapping_add(rotate(c, 30))
            .wrapping_add(d),
        a.wrapping_add(rotate(b ^ K3, 20))
            .wrapping_sub(c)
            .wrapping_add(len as u64),
    )
}

fn weak_hash_len32_with_seeds_raw(
    w: u64,
    x: u64,
    y: u64,
    z: u64,
    mut a: u64,
    mut b: u64,
) -> (u64, u64) {
    a = a.wrapping_add(w);
    b = rotate(b.wrapping_add(a).wrapping_add(z), 21);
    let c = a;
    a = a.wrapping_add(x);
    a = a.wrapping_add(y);
    b = b.wrapping_add(rotate(a, 44));
    (a.wrapping_add(z), b.wrapping_add(c))
}

fn weak_hash_len32_with_seeds(s: &[u8], at: usize, a: u64, b: u64) -> (u64, u64) {
    weak_hash_len32_with_seeds_raw(
        fetch64(s, at),
        fetch64(s, at + 8),
        fetch64(s, at + 16),
        fetch64(s, at + 24),
        a,
        b,
    )
}

fn hash_len_33_to_64(s: &[u8]) -> u64 {
    let len = s.len();
    let mut z = fetch64(s, 24);
    let mut a = fetch64(s, 0).wrapping_add(
        (len as u64)
            .wrapping_add(fetch64(s, len - 16))
            .wrapping_mul(K0),
    );
    let mut b = rotate(a.wrapping_add(z), 52);
    let mut c = rotate(a, 37);
    a = a.wrapping_add(fetch64(s, 8));
    c = c.wrapping_add(rotate(a, 7));
    a = a.wrapping_add(fetch64(s, 16));
    let vf = a.wrapping_add(z);
    let vs = b.wrapping_add(rotate(a, 31)).wrapping_add(c);
    a = fetch64(s, 16).wrapping_add(fetch64(s, len - 32));
    z = fetch64(s, len - 8);
    b = rotate(a.wrapping_add(z), 52);
    c = rotate(a, 37);
    a = a.wrapping_add(fetch64(s, len - 24));
    c = c.wrapping_add(rotate(a, 7));
    a = a.wrapping_add(fetch64(s, len - 16));
    let wf = a.wrapping_add(z);
    let ws = b.wrapping_add(rotate(a, 31)).wrapping_add(c);
    let r = shift_mix(
        vf.wrapping_add(ws)
            .wrapping_mul(K2)
            .wrapping_add(wf.wrapping_add(vs).wrapping_mul(K0)),
    );
    shift_mix(r.wrapping_mul(K0).wrapping_add(vs)).wrapping_mul(K2)
}

/// CityHash64 v1.0.x.
pub fn city_hash64(s: &[u8]) -> u64 {
    let len = s.len();
    if len <= 32 {
        return if len <= 16 {
            hash_len_0_to_16(s)
        } else {
            hash_len_17_to_32(s)
        };
    }
    if len <= 64 {
        return hash_len_33_to_64(s);
    }

    let mut x = fetch64(s, len - 40);
    let mut y = fetch64(s, len - 16).wrapping_add(fetch64(s, len - 56));
    let mut z = hash_len16(
        fetch64(s, len - 48).wrapping_add(len as u64),
        fetch64(s, len - 24),
    );
    let mut v = weak_hash_len32_with_seeds(s, len - 64, len as u64, z);
    let mut w = weak_hash_len32_with_seeds(s, len - 32, y.wrapping_add(K1), x);
    x = x.wrapping_mul(K1).wrapping_add(fetch64(s, 0));

    let mut remaining = (len - 1) & !63;
    let mut at = 0;
    loop {
        x = rotate(
            x.wrapping_add(y)
                .wrapping_add(v.0)
                .wrapping_add(fetch64(s, at + 8)),
            37,
        )
        .wrapping_mul(K1);
        y = rotate(y.wrapping_add(v.1).wrapping_add(fetch64(s, at + 48)), 42).wrapping_mul(K1);
        x ^= w.1;
        y = y.wrapping_add(v.0).wrapping_add(fetch64(s, at + 40));
        z = rotate(z.wrapping_add(w.0), 33).wrapping_mul(K1);
        v = weak_hash_len32_with_seeds(s, at, v.1.wrapping_mul(K1), x.wrapping_add(w.0));
        w = weak_hash_len32_with_seeds(
            s,
            at + 32,
            z.wrapping_add(w.1),
            y.wrapping_add(fetch64(s, at + 16)),
        );
        std::mem::swap(&mut z, &mut x);
        at += 64;
        remaining -= 64;
        if remaining == 0 {
            break;
        }
    }
    hash_len16(
        hash_len16(v.0, w.0)
            .wrapping_add(shift_mix(y).wrapping_mul(K1))
            .wrapping_add(z),
        hash_len16(v.1, w.1).wrapping_add(x),
    )
}

/// CityHash64WithSeeds v1.0.x.
pub fn city_hash64_with_seeds(s: &[u8], seed0: u64, seed1: u64) -> u64 {
    hash_len16(city_hash64(s).wrapping_sub(seed0), seed1)
}

/// CityHash64WithSeed v1.0.x.
pub fn city_hash64_with_seed(s: &[u8], seed: u64) -> u64 {
    city_hash64_with_seeds(s, K2, seed)
}
