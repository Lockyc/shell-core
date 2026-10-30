//! `<scheme>://open?…` URLs: the scripting entry point an app registers in its `Info.plist`.
//! Pure parsing; the app decides what to open. `window` names a window by title; every other
//! query pair is the app's own parameter.

use url::Url;

/// A parsed `<scheme>://open?…` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    /// The window title the `window` parameter names, if any.
    pub window: Option<String>,
    /// Every other query pair, in URL order, form-decoded.
    pub params: Vec<(String, String)>,
}

impl OpenRequest {
    /// The first value of `key` among `params`.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// The request `url` makes, or `None` unless it is `<scheme>://open…`.
pub fn open_request(url: &Url, scheme: &str) -> Option<OpenRequest> {
    if url.scheme() != scheme || url.host_str() != Some("open") {
        return None;
    }
    let mut window = None;
    let mut params = Vec::new();
    for (k, v) in url.query_pairs() {
        if k == "window" && window.is_none() {
            window = Some(v.into_owned());
        } else {
            params.push((k.into_owned(), v.into_owned()));
        }
    }
    Some(OpenRequest { window, params })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(s: &str, scheme: &str) -> Option<OpenRequest> {
        open_request(&Url::parse(s).unwrap(), scheme)
    }

    #[test]
    fn open_request_reads_window_and_params() {
        let r = req("warden://open?window=my%20work&x=1", "warden").unwrap();
        assert_eq!(r.window.as_deref(), Some("my work"));
        assert_eq!(r.params, vec![("x".to_string(), "1".to_string())]);
        let bare = req("warden://open", "warden").unwrap();
        assert_eq!((bare.window, bare.params), (None, vec![]));
    }

    #[test]
    fn other_scheme_or_host_is_none() {
        assert_eq!(req("warden://open?window=a", "flotilla"), None);
        assert_eq!(req("warden://close?window=a", "warden"), None);
        assert_eq!(req("https://open/?window=a", "warden"), None);
    }

    #[test]
    fn parses_an_encoded_open_url() {
        let r = req(
            "flotilla://open?window=Rotary%20Club&group=Bulletins&url=https%3A%2F%2Fdocs.google.com%2Fdocument%2Fd%2FA%2Fedit%3Fusp%3Dsharing%26x%3D1%2Bb%23heading%3Dh.1",
            "flotilla",
        )
        .unwrap();
        assert_eq!(r.window.as_deref(), Some("Rotary Club"));
        assert_eq!(r.param("group"), Some("Bulletins"));
        assert_eq!(
            r.param("url"),
            Some("https://docs.google.com/document/d/A/edit?usp=sharing&x=1+b#heading=h.1")
        );
    }
}
