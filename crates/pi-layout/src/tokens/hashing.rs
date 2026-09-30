//! Jenkins lookup2 string hash.
//!
//! ref: pageindex/flash/tokens/hashing.py
//!
//! The reference works on unbounded Python ints but masks every mixed value to 32 bits, so
//! wrapping `u32` arithmetic gives identical results. Its UTF-16 -> UTF-8 byte walk equals plain
//! UTF-8 for strings without lone surrogates (which Rust strings cannot hold).

/// Little-endian word of four sign-extended bytes, mod 2^32.
// ref: tokens/hashing.py::_little_endian_signed_word
fn word(b: &[u8], off: usize) -> u32 {
    let sx = |x: u8| x as i8 as i64;
    (sx(b[off]) + (sx(b[off + 1]) << 8) + (sx(b[off + 2]) << 16) + (sx(b[off + 3]) << 24)) as u32
}

// ref: tokens/hashing.py::_jenkins_mix
fn mix(s: &mut [u32; 3]) -> u32 {
    let (mut a, mut b, mut c) = (s[0], s[1], s[2]);
    a = a.wrapping_sub(b).wrapping_sub(c) ^ (c >> 13);
    b = b.wrapping_sub(c).wrapping_sub(a) ^ (a << 8);
    c = c.wrapping_sub(a);
    c = c.wrapping_sub(b) ^ (b >> 13);
    a = a.wrapping_sub(b).wrapping_sub(c) ^ (c >> 12);
    b = b.wrapping_sub(c).wrapping_sub(a) ^ (a << 16);
    c = c.wrapping_sub(a);
    c = c.wrapping_sub(b) ^ (b >> 5);
    a = a.wrapping_sub(b).wrapping_sub(c) ^ (c >> 3);
    b = b.wrapping_sub(c).wrapping_sub(a) ^ (a << 10);
    c = c.wrapping_sub(a);
    c = c.wrapping_sub(b) ^ (b >> 15);
    *s = [a, b, c];
    c
}

/// Signed 32-bit Jenkins lookup2 hash of the UTF-8 bytes (seed 314159265).
// ref: tokens/hashing.py::jenkins_hash
pub fn jenkins_hash(text: &str) -> i32 {
    let b = text.as_bytes();
    let n = b.len();
    let mut s: [u32; 3] = [0x9E3779B9, 0x9E3779B9, 314159265];
    let mut off = 0;
    let mut rem = n;
    while rem >= 12 {
        s[0] = s[0].wrapping_add(word(b, off));
        s[1] = s[1].wrapping_add(word(b, off + 4));
        s[2] = s[2].wrapping_add(word(b, off + 8));
        mix(&mut s);
        rem -= 12;
        off += 12;
    }
    s[2] = s[2].wrapping_add(n as u32);
    if rem >= 11 {
        s[2] = s[2].wrapping_add((b[off + 10] as u32) << 24);
    }
    if rem >= 10 {
        s[2] = s[2].wrapping_add((b[off + 9] as u32) << 16);
    }
    if rem >= 9 {
        s[2] = s[2].wrapping_add((b[off + 8] as u32) << 8);
    }
    if rem >= 8 {
        s[1] = s[1].wrapping_add(word(b, off + 4));
        s[0] = s[0].wrapping_add(word(b, off));
    } else if rem >= 4 {
        if rem >= 7 {
            s[1] = s[1].wrapping_add((b[off + 6] as u32) << 16);
        }
        if rem >= 6 {
            s[1] = s[1].wrapping_add((b[off + 5] as u32) << 8);
        }
        if rem >= 5 {
            s[1] = s[1].wrapping_add(b[off + 4] as u32);
        }
        s[0] = s[0].wrapping_add(word(b, off));
    } else {
        if rem >= 3 {
            s[0] = s[0].wrapping_add((b[off + 2] as u32) << 16);
        }
        if rem >= 2 {
            s[0] = s[0].wrapping_add((b[off + 1] as u32) << 8);
        }
        if rem >= 1 {
            s[0] = s[0].wrapping_add(b[off] as u32);
        }
    }
    mix(&mut s) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values from the reference `jenkins_hash` (Python 3.11).
    #[test]
    fn matches_reference() {
        for (s, want) in [
            ("", JH_EMPTY),
            ("a", JH_A),
            ("Hello, World!", JH_HELLO),
            ("Übersicht — 年次報告書 🙂 xyz", JH_UNI),
        ] {
            assert_eq!(jenkins_hash(s), want, "{s:?}");
        }
    }

    const JH_EMPTY: i32 = 1539411136;
    const JH_A: i32 = 1008794254;
    const JH_HELLO: i32 = -1940443411;
    const JH_UNI: i32 = -745119525;
}
