/// FNV-1a 64-bit -- fast, dependency-free, plenty for a cache key (not a
/// security boundary).
pub(crate) fn simple_hash(s: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
