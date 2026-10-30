//! The window set: which windows an app's config declares and which Tauri label each holds.
//! [`plan`] is the pure decision layer (labels, reconcile, menu entries); it makes no Tauri calls.
//! [`set`] is the runtime [`WindowSet`] that performs it through the app's [`WindowHost`].

pub mod flow;
pub mod form;
pub mod plan;
pub mod set;

pub use flow::{menu_request, MenuRequest};
pub use set::{RenameError, WindowHost, WindowSet, WindowWrite};
