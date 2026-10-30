//! The window set: which windows an app's config declares and which Tauri label each holds.
//! [`plan`] is the pure decision layer (labels, reconcile, menu entries); it makes no Tauri calls.

pub mod plan;
