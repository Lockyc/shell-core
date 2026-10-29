//! WebKit data-store identity. A tab's resolved `session` string (tab → window → app-wide
//! default) maps to a stable 16-byte data-store id here; tabs sharing a session string share a
//! login, distinct strings are isolated. Decoupled from window/URL, so renaming a window or
//! editing a URL never changes a login. The default session name is the app's config to own;
//! this module only hashes the seed it is handed.

use crate::geometry::fnv1a_64;

fn salted(seed: &str, salt: u8) -> u64 {
    let mut buf = Vec::with_capacity(seed.len() + 1);
    buf.push(salt);
    buf.extend_from_slice(seed.as_bytes());
    fnv1a_64(&buf)
}

/// Derive a stable 16-byte WebKit `data_store_identifier` from a session seed. Two salted
/// FNV-1a passes fill the 16 bytes.
pub fn data_store_id(seed: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&salted(seed, 0).to_le_bytes());
    out[8..].copy_from_slice(&salted(seed, 1).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_seed() {
        assert_eq!(
            data_store_id("wA:https://x/"),
            data_store_id("wA:https://x/")
        );
    }

    #[test]
    fn distinct_seeds_give_distinct_stores() {
        assert_ne!(
            data_store_id("wA:https://x/"),
            data_store_id("wB:https://x/")
        );
    }

    #[test]
    fn never_collides_with_wrys_default_zero_store() {
        assert_ne!(data_store_id("wA:https://x/"), [0u8; 16]);
    }

    /// Ids captured from curator 0.16.2's `session::data_store_id` for its default and a named
    /// session. A change here logs every user out of every service: the id names the WebKit
    /// store their cookies live in.
    #[test]
    fn data_store_id_matches_curator_0_16() {
        assert_eq!(
            data_store_id("default"),
            [8, 175, 185, 220, 5, 8, 52, 39, 225, 85, 162, 129, 20, 217, 22, 34]
        );
        assert_eq!(
            data_store_id("work"),
            [234, 92, 166, 196, 12, 212, 192, 62, 153, 209, 190, 89, 55, 16, 83, 190]
        );
    }
}
