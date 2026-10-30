//! The child-label scheme: a webview that belongs to a window (a tab, a page) is labelled
//! `{window}:{within}`. Tauri labels are app-global, so the window label namespaces them. A window
//! label may itself hold `:` (a title's `:` survives `windows::plan::sanitize_label`); a
//! within-window label never does, so the split is at the **last** `:`. Zero-dependency, so an
//! app's config crate can compose labels without the `runtime` feature.

/// The label of `within` (a `:`-free within-window name, e.g. `tab-<hash>`) inside `window`.
pub fn child_label(window: &str, within: &str) -> String {
    format!("{window}:{within}")
}

/// The window label a [`child_label`] belongs to; `None` for a label that is not a child label.
pub fn window_of(child: &str) -> Option<&str> {
    child.rsplit_once(':').map(|(w, _)| w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_label_and_window_of_round_trip_with_colons_in_titles() {
        assert_eq!(child_label("Docs", "tab-00ff"), "Docs:tab-00ff");
        for window in ["Docs", "a:b", "a:b:c", "Work-2"] {
            assert_eq!(window_of(&child_label(window, "tab-00ff-1")), Some(window));
        }
        assert_eq!(window_of("no-colon"), None);
    }
}
