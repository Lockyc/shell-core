//! Pure window plan: label assignment, config reconcile and menu entries over plain data.
//!
//! An app maps its own config window type to [`WindowDecl`] (no dependency on config-core) and
//! passes `live`: trimmed title → the Tauri label of the window currently open for it.
//!
//! - A live window can't be relabelled, so [`assign`] pins an open window to its live label and
//!   gives every other window a fresh label avoiding all live labels, [`reserved_labels`] and
//!   earlier assignments. Recomputing labels from config order alone diverges from a live window
//!   whenever two titles sanitize alike and were opened in the opposite order.
//! - [`reconcile`] matches windows by trimmed title. Width/height changes are not ops (geometry
//!   owns size after first launch). `open_on_start` is never diffed and does not gate an added
//!   window: it is a launch-only gate, so a window added or renamed (close + open) opens
//!   immediately regardless of it. Do not gate `Open` on the flag.

use std::collections::{BTreeMap, HashSet};

use crate::{home::HOME_LABEL, menu::WindowEntry};

/// A window's child labels (`{window}:{within}`); zero-dependency, so they live in [`crate::label`].
pub use crate::label::{child_label, rehome, window_of, within_of};

/// One declared window: what an app hands shell-core.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowDecl {
    /// Window title; also its identity across a reconcile (trimmed).
    pub title: String,
    /// `#rrggbb` accent, `None` = neutral.
    pub colour: Option<String>,
    /// First-run inner width, logical pixels.
    pub width: f64,
    /// First-run inner height, logical pixels.
    pub height: f64,
    /// `false` = starts closed-but-configured; consulted only at process launch.
    pub open_on_start: bool,
}

/// A declared window under its assigned label.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub label: String,
    pub decl: WindowDecl,
    /// Whether a live window currently holds `label`.
    pub open: bool,
}

/// A change to bring the running window set from `old` to `new`.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowOp {
    Open(Slot),
    /// Close the live window with this label.
    Close(String),
    /// A live window's chrome-only change (its colour).
    Update {
        label: String,
        decl: WindowDecl,
    },
}

/// Map an arbitrary title to the Tauri label charset `[A-Za-z0-9-/:_]`: other chars become `-`,
/// leading/trailing `-` are trimmed, empty becomes `window`.
pub fn sanitize_label(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | ':' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-');
    if s.is_empty() {
        "window".to_string()
    } else {
        s.to_string()
    }
}

/// Sanitize `base`, then suffix `-2`, `-3`, … until the label is neither in `taken` nor a
/// [`is_shell_label`]. A suffix can't move a label out from under the pop-out prefix, so a base
/// under it first has the prefix's `:` broken to `-`.
pub fn unique_label(base: &str, taken: &HashSet<String>) -> String {
    let mut base = sanitize_label(base);
    if crate::detach::is_detached_label(&base) {
        base = sanitize_label(&base.replacen(':', "-", 1));
    }
    std::iter::once(base.clone())
        .chain((2..).map(|n| format!("{base}-{n}")))
        .find(|c| !taken.contains(c) && !is_shell_label(c))
        .expect("unbounded range")
}

/// Whether `label` reads as one of the shell's own labels: a [`reserved_labels`] entry, a
/// pop-out's or a downloads popup's. Matched by the predicates the shell routes on, so a window
/// label can't pass them whatever its title.
pub fn is_shell_label(label: &str) -> bool {
    reserved_labels().contains(label)
        || crate::detach::is_detached_label(label)
        || crate::downloads::popup::is_popup_label(label)
}

/// Labels no declared window may take: the home surface's and the window form's, so a window
/// titled "shell home" gets `-2` instead of colliding with it (which would break geometry
/// persistence and recovery); and the pop-out prefix, which [`window_of`] reads as the window
/// label of every popped-out tab.
pub fn reserved_labels() -> HashSet<String> {
    let detach = crate::detach::DETACH_LABEL_PREFIX.trim_end_matches(':');
    HashSet::from([
        HOME_LABEL.to_string(),
        super::form::FORM_LABEL.to_string(),
        detach.to_string(),
    ])
}

/// Slots for `decls` (config order). An open window (its trimmed title in `live`) keeps its live
/// label; the rest get fresh unique labels avoiding every live label, the reserved labels and
/// labels assigned earlier. A live label already claimed by an earlier same-titled decl is not
/// reused: the later one counts as closed.
pub fn assign(decls: &[WindowDecl], live: &BTreeMap<String, String>) -> Vec<Slot> {
    let mut taken: HashSet<String> = live.values().cloned().collect();
    let mut pinned = HashSet::new();
    decls
        .iter()
        .map(|d| {
            let pin = live
                .get(d.title.trim())
                .filter(|l| pinned.insert((*l).clone()));
            let (label, open) = match pin {
                Some(l) => (l.clone(), true),
                None => {
                    let l = unique_label(&d.title, &taken);
                    taken.insert(l.clone());
                    (l, false)
                }
            };
            Slot {
                label,
                decl: d.clone(),
                open,
            }
        })
        .collect()
}

/// Whether two window titles name the same window: a window's identity is its trimmed title. The
/// one rule for every title match — [`reconcile`], [`entry_for_title`], the `WindowSet`, and any app
/// that matches its own window records by title.
pub fn title_matches(a: &str, b: &str) -> bool {
    a.trim() == b.trim()
}

fn find<'a>(decls: &'a [WindowDecl], title: &str) -> Option<&'a WindowDecl> {
    decls.iter().find(|d| title_matches(&d.title, title))
}

/// Ops to bring a running set from `old` to `new`: `Close` for a live window no longer declared
/// (once per label), `Open` for a newly declared window not already live (labelled by [`assign`]
/// against `live`), `Update` for a live kept window whose colour changed. See the module docs for
/// what is deliberately not an op.
pub fn reconcile(
    old: &[WindowDecl],
    new: &[WindowDecl],
    live: &BTreeMap<String, String>,
) -> Vec<WindowOp> {
    let mut ops = Vec::new();
    let mut closed = HashSet::new();
    for o in old {
        if find(new, &o.title).is_none() {
            if let Some(label) = live.get(o.title.trim()) {
                if closed.insert(label) {
                    ops.push(WindowOp::Close(label.clone()));
                }
            }
        }
    }
    for slot in assign(new, live) {
        match find(old, &slot.decl.title) {
            // Pinned to a live label: already open, whatever `old` says.
            None if slot.open => {}
            None => ops.push(WindowOp::Open(Slot { open: true, ..slot })),
            Some(o) if slot.open && o.colour != slot.decl.colour => ops.push(WindowOp::Update {
                label: slot.label,
                decl: slot.decl,
            }),
            Some(_) => {}
        }
    }
    ops
}

/// Window-menu / home-surface rows in config order, `id` = the slot's label.
pub fn menu_entries(slots: &[Slot]) -> Vec<WindowEntry> {
    slots
        .iter()
        .map(|s| WindowEntry {
            id: s.label.clone(),
            title: s.decl.title.clone(),
            open: s.open,
            colour: s.decl.colour.clone(),
        })
        .collect()
}

/// The entry of the window titled `title` (trimmed): its current label and whether it is open.
/// How an app finds a popped-out tab's origin window, which it records by title because a label
/// can't follow an in-place rename.
pub fn entry_for_title<'a>(entries: &'a [WindowEntry], title: &str) -> Option<&'a WindowEntry> {
    entries.iter().find(|e| title_matches(&e.title, title))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(title: &str) -> WindowDecl {
        WindowDecl {
            title: title.into(),
            colour: Some("#000000".into()),
            width: 800.0,
            height: 600.0,
            open_on_start: true,
        }
    }

    fn live(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(t, l)| (t.to_string(), l.to_string()))
            .collect()
    }

    fn labels(slots: &[Slot]) -> Vec<&str> {
        slots.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn sanitize_maps_to_label_charset() {
        assert_eq!(sanitize_label("  a b!  "), "a-b");
        assert_eq!(sanitize_label("!!!"), "window");
    }

    #[test]
    fn unique_label_suffixes_on_collision() {
        let taken = HashSet::from(["work".to_string(), "work-2".to_string()]);
        assert_eq!(unique_label("work", &taken), "work-3");
    }

    #[test]
    fn assign_dedupes_labels_for_colliding_sanitized_names() {
        let slots = assign(&[decl("a b"), decl("a-b")], &BTreeMap::new());
        assert_eq!(labels(&slots), ["a-b", "a-b-2"]);
        assert!(slots.iter().all(|s| !s.open));
    }

    #[test]
    fn assign_pins_open_windows_to_their_live_labels() {
        let slots = assign(
            &[decl("a b"), decl("a-b")],
            &live(&[("a b", "a-b-2"), ("a-b", "a-b")]),
        );
        assert_eq!(labels(&slots), ["a-b-2", "a-b"]);
        assert!(slots.iter().all(|s| s.open));
    }

    #[test]
    fn assign_gives_closed_windows_labels_avoiding_live_ones() {
        let slots = assign(&[decl("a b"), decl("a-b")], &live(&[("a b", "a-b-2")]));
        assert_eq!(labels(&slots), ["a-b-2", "a-b"]);
        assert!(slots[0].open && !slots[1].open);
    }

    #[test]
    fn assign_reserves_home_label_for_closed_windows() {
        let slots = assign(&[decl("shell home")], &BTreeMap::new());
        assert_ne!(slots[0].label, HOME_LABEL);
        assert_eq!(slots[0].label, "shell-home-2");
    }

    #[test]
    fn assign_matches_live_by_trimmed_title() {
        let slots = assign(&[decl(" work ")], &live(&[("work", "work")]));
        assert!(slots[0].open);
    }

    #[test]
    fn menu_entries_preserve_order_and_tag_open_state() {
        let slots = assign(
            &[decl("work"), decl("side project")],
            &live(&[("work", "work")]),
        );
        let entries = menu_entries(&slots);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "work");
        assert_eq!(entries[0].title, "work");
        assert!(entries[0].open);
        assert_eq!(entries[1].id, "side-project");
        assert!(!entries[1].open);
        assert_eq!(entries[0].colour.as_deref(), Some("#000000"));
        let found = entry_for_title(&entries, " side project ").unwrap();
        assert_eq!((found.id.as_str(), found.open), ("side-project", false));
        assert!(entry_for_title(&entries, "gone").is_none());
    }

    #[test]
    fn titles_match_ignoring_surrounding_whitespace_only() {
        assert!(title_matches(" side project ", "side project"));
        assert!(title_matches("Work", "Work"));
        assert!(!title_matches("Work", "work"));
        assert!(!title_matches("side  project", "side project"));
    }

    #[test]
    fn a_window_never_takes_the_pop_out_prefix_as_its_label() {
        // `window_of("shell-detach:<token>")` is `shell-detach`: a window holding that label would
        // own every pop-out as its child.
        let slots = assign(&[decl("shell detach")], &BTreeMap::new());
        assert_eq!(slots[0].label, "shell-detach-2");
    }

    #[test]
    fn added_window_opens_even_if_open_on_start_false() {
        let mut added = decl("side");
        added.open_on_start = false;
        let ops = reconcile(
            &[decl("work")],
            &[decl("work"), added],
            &live(&[("work", "work")]),
        );
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            WindowOp::Open(s) => assert!(s.open && s.label == "side"),
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[test]
    fn removed_window_closes() {
        let ops = reconcile(
            &[decl("work"), decl("side")],
            &[decl("work")],
            &live(&[("work", "work"), ("side", "side")]),
        );
        assert_eq!(ops, [WindowOp::Close("side".into())]);
    }

    #[test]
    fn removed_closed_window_is_no_op() {
        let ops = reconcile(
            &[decl("work"), decl("side")],
            &[decl("work")],
            &live(&[("work", "work")]),
        );
        assert!(ops.is_empty());
    }

    #[test]
    fn colour_change_is_an_update() {
        let mut new = decl("work");
        new.colour = Some("#ffffff".into());
        let ops = reconcile(&[decl("work")], &[new.clone()], &live(&[("work", "work")]));
        assert_eq!(
            ops,
            [WindowOp::Update {
                label: "work".into(),
                decl: new
            }]
        );
    }

    #[test]
    fn size_and_open_on_start_changes_are_not_ops() {
        let mut new = decl("work");
        new.width = 1.0;
        new.height = 2.0;
        new.open_on_start = false;
        assert!(reconcile(&[decl("work")], &[new], &live(&[("work", "work")])).is_empty());
    }

    #[test]
    fn assign_lets_only_the_first_same_titled_decl_claim_a_live_label() {
        let slots = assign(&[decl("side"), decl("side")], &live(&[("side", "side")]));
        assert_eq!(labels(&slots), ["side", "side-2"]);
        assert!(slots[0].open && !slots[1].open);
    }

    #[test]
    fn added_title_already_live_is_not_reopened() {
        let ops = reconcile(&[], &[decl("work")], &live(&[("work", "work")]));
        assert!(ops.is_empty(), "{ops:?}");
    }

    #[test]
    fn duplicate_titled_removed_decls_close_once() {
        let ops = reconcile(
            &[decl("side"), decl("side")],
            &[],
            &live(&[("side", "side")]),
        );
        assert_eq!(ops, [WindowOp::Close("side".into())]);
    }

    mod props {
        use super::*;
        use crate::detach::is_detached_label;
        use crate::downloads::popup::{is_popup_label, POPUP_WITHIN};
        use crate::windows::form::FORM_LABEL;
        use proptest::prelude::*;

        /// Titles built from the words and separators the shell's own labels are made of, so
        /// the generator reaches every reserved shape in a handful of cases.
        fn title() -> impl Strategy<Value = String> {
            let piece = prop::sample::select(vec![
                "shell detach",
                "shell downloads",
                "shell home",
                "shell window form",
                "shell",
                "home",
                "detach",
                "downloads",
                "window",
                "form",
                "x",
                ":",
                " ",
                "-",
            ]);
            prop::collection::vec(piece, 0..5).prop_map(|p| p.concat())
        }

        fn live_map() -> impl Strategy<Value = BTreeMap<String, String>> {
            prop::collection::btree_map(title(), title(), 0..3).prop_map(|m| {
                // A live label was itself assigned, so it is never a shell label.
                let decls: Vec<_> = m.keys().map(|t| decl(t)).collect();
                assign(&decls, &BTreeMap::new())
                    .into_iter()
                    .map(|s| (s.decl.title.trim().to_string(), s.label))
                    .collect()
            })
        }

        proptest! {
            #[test]
            fn no_title_takes_a_shell_label(
                titles in prop::collection::vec(title(), 0..6),
                live in live_map(),
            ) {
                let decls: Vec<_> = titles.iter().map(|t| decl(t)).collect();
                let slots = assign(&decls, &live);
                let mut seen = HashSet::new();
                for s in &slots {
                    let l = s.label.as_str();
                    prop_assert!(!is_detached_label(l), "{l:?} reads as a pop-out");
                    prop_assert!(!is_popup_label(l), "{l:?} reads as a downloads popup");
                    prop_assert_ne!(l, HOME_LABEL);
                    prop_assert_ne!(l, FORM_LABEL);
                    prop_assert!(seen.insert(l), "{l:?} assigned twice");
                }
            }

            #[test]
            fn a_child_belongs_to_its_window(
                t in title(),
                within in "[a-z-]{1,12}|shell-downloads",
            ) {
                let w = assign(&[decl(&t)], &BTreeMap::new()).remove(0).label;
                let child = child_label(&w, &within);
                prop_assert_eq!(window_of(&child), Some(w.as_str()));
                prop_assert_eq!(is_popup_label(&child), within == POPUP_WITHIN);
                prop_assert!(!is_detached_label(&child));
            }
        }
    }

    #[test]
    fn rename_is_close_plus_open() {
        let ops = reconcile(&[decl("work")], &[decl("job")], &live(&[("work", "work")]));
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0], WindowOp::Close("work".into()));
        assert!(matches!(&ops[1], WindowOp::Open(s) if s.label == "job" && s.open));
    }
}
