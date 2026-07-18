//! Shared tab-selection policy. One decision, used identically by every app: after a tab is
//! unloaded, which sibling (if any) becomes active. What counts as "loaded" is each app's own
//! concern (a spawned terminal surface, a created webview, a live doc server) — this function
//! only takes the ordered loaded-state and the index being unloaded, so it is window-agnostic and
//! pure. It lives here (not in chrome-core's JS) because warden auto-unloads a tab from Rust when
//! its child process exits, with no chrome round-trip to hook.

/// Index of the tab to activate after the tab at `idx` is unloaded, given each tab's loaded state
/// in tab order. Lean **up** the list: take the nearest loaded tab to the left (the one you
/// usually came from), else the nearest loaded tab to the right. `None` ⇒ nothing loaded to show —
/// the caller blanks the hole rather than waking a cold tab. Pure index logic, unit-testable
/// without real surfaces/servers.
pub fn pick_live_neighbour(idx: usize, eligible: &[bool]) -> Option<usize> {
    if let Some(p) = (0..idx).rev().find(|&i| eligible[i]) {
        return Some(p);
    }
    ((idx + 1)..eligible.len()).find(|&i| eligible[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_live_neighbour_prefers_previous_when_live() {
        // unloaded@2; previous@1 is loaded → take it (lean up), even though next@3 is also loaded.
        assert_eq!(pick_live_neighbour(2, &[false, true, true, true]), Some(1));
    }

    #[test]
    fn pick_live_neighbour_prefers_nearest_live_left_over_right() {
        // unloaded@3; nearest loaded left is @1 (@2 cold), loaded far right @4 → left wins.
        assert_eq!(
            pick_live_neighbour(3, &[false, true, false, false, true]),
            Some(1)
        );
    }

    #[test]
    fn pick_live_neighbour_uses_right_when_nothing_live_left() {
        // unloaded@1; nothing loaded to the left (@0 cold); loaded@3 → scan right to it.
        assert_eq!(
            pick_live_neighbour(1, &[false, false, false, true]),
            Some(3)
        );
    }

    #[test]
    fn pick_live_neighbour_none_when_nothing_live() {
        // No loaded tab anywhere → blank the hole, never spawn one to fill it.
        assert_eq!(pick_live_neighbour(1, &[false, false, false]), None);
    }
}
