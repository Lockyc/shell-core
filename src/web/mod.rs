//! App-agnostic content-webview layer shared by apps that host remote sites in WKWebViews: link
//! routing and escape sentinels ([`escape`]), WebKit login-store identity ([`session`]), and
//! allowlisted self-signed TLS ([`insecure`], macOS).

pub mod escape;
#[cfg(target_os = "macos")]
pub mod insecure;
pub mod session;
