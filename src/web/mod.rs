//! App-agnostic content-webview layer shared by apps that host remote sites in WKWebViews: link
//! routing and escape sentinels ([`escape`]), WebKit login-store identity ([`session`]),
//! allowlisted self-signed TLS ([`insecure`], macOS), keeping the Web Inspector off the sidebar
//! ([`inspector`], macOS), leaving the page's `ipc` global free ([`ipc_global`], macOS), and the
//! content-webview builder ([`content`]).

pub mod content;
pub mod escape;
#[cfg(target_os = "macos")]
pub mod insecure;
#[cfg(target_os = "macos")]
pub mod inspector;
#[cfg(target_os = "macos")]
pub mod ipc_global;
pub mod session;
