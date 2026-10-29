/// A same-tab/main-frame navigation: allow it. Link clicks never reach this as a cross-site
/// navigation — the escape-click shim routes those through [`route_link`] first — so what
/// arrives here is the page's own navigation (redirects, scripted `location` changes, SSO
/// bounces), which must be free to follow.
pub fn allow_same_tab_navigation(_url: &str) -> bool {
    true
}

/// Where a link the user followed out of a tab goes.
#[derive(Debug, PartialEq, Eq)]
pub enum LinkRoute {
    /// The tab's own site (or its provider's auth host): navigate the tab itself.
    InApp(url::Url),
    /// Anywhere else: hand to the default browser.
    Browser(url::Url),
    /// A scheme the shell won't hand to the opener ([`is_escapable_scheme`]): drop it.
    Refuse,
}

/// The one routing decision for a link leaving a tab — a new-window request and a plain
/// cross-site click both come through here. Google's link redirector is unwrapped first so the
/// decision is made on the real destination ([`redirector_target`]).
pub fn route_link(home_url: &str, url: url::Url) -> LinkRoute {
    let url = redirector_target(&url).unwrap_or(url);
    if same_site(home_url, &url) {
        LinkRoute::InApp(url)
    } else if is_escapable_scheme(&url) {
        LinkRoute::Browser(url)
    } else {
        LinkRoute::Refuse
    }
}

/// Build the argv for handing a URL to the macOS default handler (the user's default browser).
pub fn open_command(url: &str) -> (&'static str, Vec<String>) {
    ("open", vec![url.to_string()])
}

/// Hand a URL to the macOS default handler (the user's default browser). Side-effecting; not unit-tested.
pub fn escape_to_default_browser(url: &str) {
    let (cmd, args) = open_command(url);
    let _ = std::process::Command::new(cmd).args(args).spawn();
}

/// Schemes the shell will hand to the macOS opener for an in-app `window.open` / new-window
/// intent. http/https are normal links; mailto/tel are the everyday "open in my mail/phone
/// app" hand-offs. Everything else (file://, custom app schemes, smb://, …) is refused so a
/// page can't drive `open` into launching an arbitrary handler.
pub fn is_escapable_scheme(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "mailto" | "tel")
}

/// A host's registrable domain by the public-suffix list (`accounts.google.com` → `google.com`,
/// `login.phish.com.au` → `phish.com.au`, never `com.au`). `None` for a host with no registrable
/// part (an IP, a bare suffix), which therefore matches nothing.
fn registrable(host: &str) -> Option<&str> {
    psl::domain_str(host)
}

/// Whether `host` is one of Google's own hosts (`www.google.com`, `google.com.au`, …): an
/// optional `www.` prefix followed by a `google.` label. Used only to recognise the link
/// redirector below, so it deliberately doesn't try to be a general Google-property test.
fn is_google_host(host: &str) -> bool {
    host.strip_prefix("www.")
        .unwrap_or(host)
        .starts_with("google.")
}

/// Google's link redirector — `https://www.google.com/url?q=<target>` — which Google Chat and
/// Gmail interpose on every external link a message contains. Its whole purpose is to *leave*,
/// so it is unwrapped to its real destination before any routing decision runs. That matters in
/// both directions: a wrapper clicked as a plain same-tab navigation would otherwise be waved
/// through by [`allow_same_tab_navigation`] and bounce the tab out to the external site, while a
/// wrapper pointing back at the tab's own host is ordinary in-app navigation that must not
/// escape. Unwrapping also hands the browser the clean URL rather than Google's interstitial.
///
/// Returns the target of `url`'s `q` (or `url`) param when `url` is such a redirector and that
/// target is http(s); any other URL — or a redirector with a missing/garbage target — yields
/// `None`, leaving the navigation to be classified as it was.
pub fn redirector_target(url: &url::Url) -> Option<url::Url> {
    if url.path() != "/url" || !url.host_str().is_some_and(is_google_host) {
        return None;
    }
    let target = url
        .query_pairs()
        .find(|(k, _)| k == "q" || k == "url")
        .map(|(_, v)| v.into_owned())?;
    let target = url::Url::parse(&target).ok()?;
    matches!(target.scheme(), "http" | "https").then_some(target)
}

/// A host with any leading `www.` dropped, so `www.example.com` and `example.com` compare equal.
fn bare(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

/// Whether `host`'s leading label names an authentication surface (`accounts.google.com`,
/// `login.example.com`, `sso.example.org`, …). Deliberately a small, bounded set of *auth*
/// labels rather than an open-ended list of a provider's app hosts — see [`same_site`].
fn is_auth_host(host: &str) -> bool {
    matches!(
        host.split('.').next().unwrap_or_default(),
        "accounts" | "account" | "login" | "signin" | "auth" | "oauth" | "sso" | "id" | "secure"
    )
}

/// Whether a new-window `target` belongs to the same site as the tab's `home_url` — i.e. it's
/// the app's own flow (a sign-in popup goes to the provider's auth host) rather than an external
/// link. Same-site new windows are kept in-app so they complete in the tab's own login session;
/// cross-site ones escape to the default browser. http(s) only.
///
/// "Same site" is the tab's **own host** (`www.`-insensitive), not its whole registrable domain.
/// A shared provider domain hosts many *unrelated services* — `meet.google.com` and
/// `docs.google.com` are no more the Chat tab's own flow than an outside link is, and treating
/// them as such navigated the tab away from Chat instead of opening them in the browser. The one
/// carve-out is an auth host on the same registrable domain ([`is_auth_host`]), which is the
/// sign-in popup this test exists to keep in-app.
pub fn same_site(home_url: &str, target: &url::Url) -> bool {
    if !matches!(target.scheme(), "http" | "https") {
        return false;
    }
    let (Some(t_host), Ok(home)) = (target.host_str(), url::Url::parse(home_url)) else {
        return false;
    };
    let Some(h_host) = home.host_str() else {
        return false;
    };
    if bare(h_host) == bare(t_host) {
        return true;
    }
    registrable(h_host).is_some_and(|h| registrable(t_host) == Some(h)) && is_auth_host(t_host)
}

/// Sentinel host the injected click-interceptor navigates to so the native
/// `on_navigation` handler can see link clicks (which WKWebView otherwise delivers as ordinary
/// main-frame navigations, indistinguishable from the page's own redirects). Must be a host no
/// real keeper site will ever use. An app's own sentinels (notify, badge, …) are its to define;
/// [`is_sentinel_host`] recognises any `.invalid` host.
pub const ESCAPE_SENTINEL_HOST: &str = "shell.escape.invalid";

/// A link click decoded from the escape sentinel.
#[derive(Debug, PartialEq, Eq)]
pub struct LinkClick {
    pub target: url::Url,
    /// `false`: cmd/middle-click — an explicit "open elsewhere", always the browser.
    /// `true` (`r` param): a plain click, routed by [`route_link`].
    pub routed: bool,
}

/// If `nav_url` is the escape sentinel, decode the click: its `u` param (restricted to
/// http/https) and whether it is routed. Any other navigation — or a sentinel with a
/// missing/garbage target — yields `None` (don't act on junk).
pub fn escape_sentinel(nav_url: &url::Url) -> Option<LinkClick> {
    if nav_url.host_str() != Some(ESCAPE_SENTINEL_HOST) {
        return None;
    }
    let param = |key: &str| {
        nav_url
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    let target = url::Url::parse(&param("u")?).ok()?;
    matches!(target.scheme(), "http" | "https").then(|| LinkClick {
        target,
        routed: param("r").is_some(),
    })
}

/// True if `url`'s host is an internal sentinel host: any `.invalid` host (RFC 6761 reserves the
/// TLD, so no real site can collide), covering the escape sentinel and every app-defined one.
pub fn is_sentinel_host(url: &url::Url) -> bool {
    url.host_str().is_some_and(|h| h.ends_with(".invalid"))
}

/// Whether a sentinel `url` carries this webview's secret `expected` key (the `k` param). Our
/// injected shims bake the key in as a function-local literal — never on `window` — so a page
/// can't read it and therefore can't forge a sentinel navigation by hitting the host directly.
pub fn sentinel_key_ok(url: &url::Url, expected: &str) -> bool {
    !expected.is_empty() && url.query_pairs().any(|(k, v)| k == "k" && v == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_tab_navigation_is_always_allowed() {
        assert!(allow_same_tab_navigation(
            "https://mail.google.com/mail/u/0/#inbox"
        ));
        assert!(allow_same_tab_navigation("https://example.com/whatever"));
    }

    #[test]
    fn open_command_shells_to_macos_open() {
        let (cmd, args) = open_command("https://example.com/");
        assert_eq!(cmd, "open");
        assert_eq!(args, vec!["https://example.com/".to_string()]);
    }

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn sentinel_extracts_encoded_target() {
        let u = url("https://shell.escape.invalid/?u=https%3A%2F%2Fexample.org%2Fa%3Fb%3Dc");
        assert_eq!(
            escape_sentinel(&u),
            Some(LinkClick {
                target: url("https://example.org/a?b=c"),
                routed: false
            })
        );
    }

    #[test]
    fn sentinel_r_param_marks_a_routed_click() {
        let u = url("https://shell.escape.invalid/?u=https%3A%2F%2Fexample.org%2F&r=1&k=x");
        assert!(escape_sentinel(&u).unwrap().routed);
    }

    #[test]
    fn route_link_keeps_own_site_and_sends_the_rest_to_the_browser() {
        let home = "https://chat.google.com/";
        assert_eq!(
            route_link(home, url("https://chat.google.com/room/x")),
            LinkRoute::InApp(url("https://chat.google.com/room/x"))
        );
        assert_eq!(
            route_link(home, url("https://example.com/a")),
            LinkRoute::Browser(url("https://example.com/a"))
        );
        // Decided on the redirector's real destination, not the wrapper.
        assert_eq!(
            route_link(
                home,
                url("https://www.google.com/url?q=https%3A%2F%2Fexample.com%2Fa")
            ),
            LinkRoute::Browser(url("https://example.com/a"))
        );
        assert_eq!(
            route_link(home, url("file:///etc/passwd")),
            LinkRoute::Refuse
        );
    }

    #[test]
    fn non_sentinel_url_is_none() {
        let u = url("https://mail.google.com/mail/u/0/#inbox");
        assert_eq!(escape_sentinel(&u), None);
    }

    #[test]
    fn sentinel_without_u_is_none() {
        let u = url("https://shell.escape.invalid/");
        assert_eq!(escape_sentinel(&u), None);
    }

    #[test]
    fn sentinel_with_non_http_target_is_none() {
        let u = url("https://shell.escape.invalid/?u=file%3A%2F%2F%2Fetc%2Fpasswd");
        assert_eq!(escape_sentinel(&u), None);
    }

    #[test]
    fn same_site_keeps_provider_auth_in_app() {
        // Google Chat opening its accounts domain is same registrable domain → in-app.
        assert!(same_site(
            "https://chat.google.com/",
            &url("https://accounts.google.com/o/oauth2/auth?foo=bar")
        ));
        // Exact-host popup is same-site too.
        assert!(same_site(
            "https://app.element.io/",
            &url("https://app.element.io/#/login")
        ));
    }

    #[test]
    fn same_site_escapes_sibling_services_on_a_shared_provider_domain() {
        // Google Chat's "Join video meeting" card is a `target="_blank"` link to meet.google.com.
        // It shares `google.com` with the tab, but it is a different *service*, not the tab's own
        // auth flow — so it must escape to the browser rather than navigate the Chat tab away.
        assert!(!same_site(
            "https://chat.google.com/",
            &url("https://meet.google.com/abc-defg-hij")
        ));
        assert!(!same_site(
            "https://chat.google.com/",
            &url("https://docs.google.com/document/d/1/edit")
        ));
    }

    #[test]
    fn same_site_escapes_external_links() {
        // A genuinely external link is cross-site → not kept in-app.
        assert!(!same_site(
            "https://chat.google.com/",
            &url("https://example.com/article")
        ));
        // Non-web schemes are never "same site".
        assert!(!same_site(
            "https://chat.google.com/",
            &url("mailto:a@b.test")
        ));
    }

    #[test]
    fn redirector_unwraps_googles_link_wrapper() {
        // What Google Chat actually hands the new-window handler for an external link.
        let u = url(
            "https://www.google.com/url?q=https%3A%2F%2Ftracker.test%2Fgantt%3Fcharts%3D01k&sa=D&usg=AOv",
        );
        assert_eq!(
            redirector_target(&u).map(|t| t.to_string()),
            Some("https://tracker.test/gantt?charts=01k".to_string())
        );
        // Country domains and the bare host wrap links the same way.
        assert!(
            redirector_target(&url("https://google.com.au/url?q=https%3A%2F%2Fx.test%2F"))
                .is_some()
        );
        // The `url` param form is the same redirector.
        assert!(redirector_target(&url(
            "https://www.google.com/url?url=https%3A%2F%2Fx.test%2F"
        ))
        .is_some());
    }

    #[test]
    fn redirector_ignores_everything_else() {
        // Not the redirect endpoint — an OAuth popup carries an absolute `redirect_uri` and
        // must stay in-app, so only the `/url` path counts as a redirector.
        assert!(redirector_target(&url(
            "https://accounts.google.com/o/oauth2/auth?redirect_uri=https%3A%2F%2Fapp.test%2Fcb"
        ))
        .is_none());
        // Not a Google host.
        assert!(
            redirector_target(&url("https://notgoogle.test/url?q=https%3A%2F%2Fx.test%2F"))
                .is_none()
        );
        // Missing or non-http target → leave the navigation alone.
        assert!(redirector_target(&url("https://www.google.com/url")).is_none());
        assert!(
            redirector_target(&url("https://www.google.com/url?q=javascript%3Aalert(1)")).is_none()
        );
    }

    #[test]
    fn unwrapped_redirector_is_routed_on_its_real_destination() {
        // A wrapped external link resolves to a cross-site target, so it escapes to the browser
        // — and the browser gets the clean URL, not Google's interstitial.
        let wrapper = url("https://www.google.com/url?q=https%3A%2F%2Ftracker.test%2Fgantt");
        let target = redirector_target(&wrapper).unwrap();
        assert_eq!(target.as_str(), "https://tracker.test/gantt");
        assert!(!same_site("https://chat.google.com/", &target));
        assert!(is_escapable_scheme(&target));

        // The other direction: a wrapper pointing back at the tab's own host is in-app
        // navigation, and only unwrapping reveals that (the wrapper's own host is not the tab's).
        let inward = url("https://www.google.com/url?q=https%3A%2F%2Fchat.google.com%2Froom%2Fx");
        assert!(!same_site("https://chat.google.com/", &inward));
        assert!(same_site(
            "https://chat.google.com/",
            &redirector_target(&inward).unwrap()
        ));
    }

    #[test]
    fn same_site_keeps_auth_hosts_and_www_variants_in_app() {
        // A sign-in popup on the provider's auth host is the tab's own flow.
        assert!(same_site(
            "https://git.example.org/",
            &url("https://sso.example.org/login")
        ));
        assert!(same_site(
            "https://app.example.com/",
            &url("https://login.example.com/oauth/authorize")
        ));
        // `www.` is not a meaningful host difference.
        assert!(same_site(
            "https://www.notion.so/",
            &url("https://notion.so/x")
        ));
        assert!(same_site(
            "https://notion.so/",
            &url("https://www.notion.so/x")
        ));
        // An auth-looking host on a *different* registrable domain is still external.
        assert!(!same_site(
            "https://chat.google.com/",
            &url("https://login.example.com/")
        ));
        // A two-part public suffix is not a shared domain: anyone can own `phish.com.au`.
        assert!(!same_site(
            "https://mail.example.com.au/",
            &url("https://login.phish.com.au/")
        ));
        assert!(same_site(
            "https://mail.example.com.au/",
            &url("https://login.example.com.au/")
        ));
    }

    #[test]
    fn escapable_schemes_are_web_and_contact() {
        for s in [
            "https://x.test/",
            "http://x.test/",
            "mailto:a@b.test",
            "tel:+15551234",
        ] {
            assert!(is_escapable_scheme(&url(s)), "{s} should escape");
        }
    }

    #[test]
    fn non_escapable_schemes_are_refused() {
        for s in [
            "file:///etc/passwd",
            "smb://server/share",
            "ftp://x.test/f",
            "customapp://do-something",
        ] {
            assert!(!is_escapable_scheme(&url(s)), "{s} should be refused");
        }
    }

    #[test]
    fn any_invalid_host_is_a_sentinel() {
        assert!(is_sentinel_host(&url(
            "https://curator.notify.invalid/?t=x"
        )));
        assert!(is_sentinel_host(&url("https://shell.escape.invalid/?u=x")));
        assert!(!is_sentinel_host(&url("https://example.com/")));
        assert!(!is_sentinel_host(&url("https://mail.google.com/")));
    }

    #[test]
    fn escape_sentinel_reads_target_and_routed_flag() {
        let u = url("https://shell.escape.invalid/?u=https%3A%2F%2Fx.com%2F&r=1&k=K");
        assert_eq!(
            escape_sentinel(&u),
            Some(LinkClick {
                target: url("https://x.com/"),
                routed: true
            })
        );
    }

    #[test]
    fn sentinel_key_must_match() {
        let u = url("https://shell.escape.invalid/?t=x&k=abc123");
        assert!(sentinel_key_ok(&u, "abc123"));
        assert!(!sentinel_key_ok(&u, "wrong"));
        // A missing key never matches (a forged sentinel with no key is rejected).
        assert!(!sentinel_key_ok(
            &url("https://shell.escape.invalid/?t=x"),
            "abc123"
        ));
        // An empty expected key never matches, even against a keyless URL.
        assert!(!sentinel_key_ok(&u, ""));
    }
}
