//! The detached-window label scheme: how shell-core marks a window as an ephemeral "popped out"
//! tab so hot-reload reconcile and window-state persistence both know to skip it (mirroring the
//! home surface's own exclusion via [`crate::home::HOME_LABEL`]). This module currently owns only
//! the label scheme + its reconcile-skip predicate — the banner-shell surface and the
//! detach/return orchestration that build on it land in later work.
//!
//! Touches no config-core symbol — the cores stay mutually independent.

/// Prefix marking a window label as a detached-tab window. A label under this prefix is never a
/// real (config-defined) window label, so reconcile and window-state persistence can both use
/// [`is_detached_label`] to skip it — the same exclusion `home::HOME_LABEL` gets, generalized to
/// an unbounded set of ephemeral windows (one per detached tab) rather than a single fixed label.
pub const DETACH_LABEL_PREFIX: &str = "shell-detach:";

/// Build the Tauri window label for a detached tab identified by `token` (an opaque,
/// caller-chosen identifier — e.g. the tab's own key).
pub fn detached_label(token: &str) -> String {
    format!("{DETACH_LABEL_PREFIX}{token}")
}

/// Whether `label` names a detached-tab window (as opposed to a real config-defined window or the
/// home surface). Reconcile and window-state persistence use this to skip these windows.
pub fn is_detached_label(label: &str) -> bool {
    label.starts_with(DETACH_LABEL_PREFIX)
}

/// The inverse of [`detached_label`]: recover the token from a detached-tab label, for routing an
/// event/command back to the right tab. `None` if `label` isn't a detached-tab label.
pub fn detach_token(label: &str) -> Option<&str> {
    label.strip_prefix(DETACH_LABEL_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_round_trip() {
        let l = detached_label("abc123");
        assert_eq!(l, "shell-detach:abc123");
        assert!(is_detached_label(&l));
        assert_eq!(detach_token(&l), Some("abc123"));
    }

    #[test]
    fn real_window_labels_are_not_detached() {
        assert!(!is_detached_label("w1a2b3"));
        assert!(!is_detached_label(super::super::home::HOME_LABEL));
        assert_eq!(detach_token("w1a2b3"), None);
    }
}
