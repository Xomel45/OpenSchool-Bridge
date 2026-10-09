use std::sync::Arc;

use reqwest::cookie::Jar;

use crate::datamart::{self, Query};
use crate::error::BridgeError;
use crate::models::{ClassInfo, Student, Week};

const DEFAULT_BASE_URL: &str = "https://www.gosuslugi.ru";
/// The OS family the bridge was built for: only the family, no version or device (that would be a fingerprint).
#[cfg(target_os = "android")]
macro_rules! platform { () => { "Android" }; }
#[cfg(target_os = "windows")]
macro_rules! platform { () => { "Windows" }; }
#[cfg(target_os = "linux")]
macro_rules! platform { () => { "Linux" }; }
#[cfg(target_os = "macos")]
macro_rules! platform { () => { "macOS" }; }
#[cfg(target_os = "ios")]
macro_rules! platform { () => { "iOS" }; }
#[cfg(not(any(target_os = "android", target_os = "windows", target_os = "linux", target_os = "macos", target_os = "ios")))]
macro_rules! platform { () => { "Unknown" }; }

/// An honest client identifier, `OpenSchool/<version> (<OS family>; +<repository>)`; we do not pretend to be a browser.
const USER_AGENT: &str = concat!("OpenSchool/", env!("CARGO_PKG_VERSION"), " (", platform!(), "; +https://github.com/Xomel45/OpenSchool-Bridge)");

/// Where a UI should send the user to log in: Gosuslugi redirects to ESIA and, once the user is
/// authenticated, lands back on the diary.
pub const LOGIN_URL: &str = "https://www.gosuslugi.ru/school/feed";

/// A cookie of the Gosuslugi session, as captured by the UI after the ESIA login.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SessionCookie {
    pub name: String,
    pub value: String,
}

/// A cookie we are willing to send: a name, and no `;` or line breaks in either part (they would become cookie attributes
/// or extra headers).
fn plain_cookie(c: &SessionCookie) -> bool {
    let bad = |s: &str| s.contains([';', '\r', '\n']);
    !c.name.is_empty() && !bad(&c.name) && !bad(&c.value)
}

/// Split a `Cookie`-style header (`"a=1; b=2"`, as returned by Android's `CookieManager`).
/// Malformed pieces are skipped.
fn parse_cookie_header(header: &str) -> Vec<SessionCookie> {
    header
        .split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            let name = name.trim();
            (!name.is_empty()).then(|| SessionCookie { name: name.to_string(), value: value.trim().to_string() })
        })
        .collect()
}

/// Cookies and tokens stay inside the core and are never exposed back to UI code.
#[derive(uniffi::Object)]
pub struct Client {
    http: reqwest::Client,
    jar: Arc<Jar>,
    base_url: reqwest::Url,
}

/// A dead or stalled connection must end with an error, not with a spinner that never stops.
/// (Tests use a short limit.)
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const REQUEST_TIMEOUT: std::time::Duration = if cfg!(test) { std::time::Duration::from_millis(400) } else { std::time::Duration::from_secs(30) };

/// The session cookies go wherever this URL points, so it must be https; plain http only for a server on this machine (tests).
fn check_base_url(url: &reqwest::Url) -> Result<(), BridgeError> {
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => Ok(()),
        "http" if local => Ok(()),
        _ => Err(BridgeError::Network("base URL must use https (http is allowed only for localhost)".into())),
    }
}

/// The year bound only rejects obvious garbage before it reaches the server.
fn check_year(year: u32) -> Result<(), BridgeError> {
    if (2000..=2100).contains(&year) { Ok(()) } else { Err(BridgeError::Parse(format!("invalid year {year}"))) }
}

/// 52 or 53: a year has 53 ISO weeks when 1 January is a Thursday, or a Wednesday in a leap year.
fn iso_weeks_in(year: u32) -> u32 {
    let p = |y: u32| (y + y / 4 - y / 100 + y / 400) % 7;
    if p(year) == 4 || p(year.saturating_sub(1)) == 3 { 53 } else { 52 }
}

fn check_week(year: u32, iso_week: u32) -> Result<(), BridgeError> {
    check_year(year)?;
    if (1..=iso_weeks_in(year)).contains(&iso_week) { Ok(()) } else { Err(BridgeError::Parse(format!("invalid week {iso_week} of {year}"))) }
}

/// Services that tell the caller its country. Plain https, no keys. Order = preference.
const GEO_SERVICES: [&str; 3] = ["https://ipinfo.io/country", "https://ipapi.co/country/", "https://api.country.is/"];

/// `{"ip":"..","country":"DE"}` or a bare `DE` (surrounding whitespace allowed) -> `DE`.
fn parse_country(text: &str) -> Option<String> {
    let t = text.trim();
    let code = match serde_json::from_str::<serde_json::Value>(t) {
        Ok(v) => v.get("country").and_then(|c| c.as_str()).map(str::to_string)?,
        Err(_) => t.to_string(),
    };
    (code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic())).then(|| code.to_ascii_uppercase())
}

fn build_client(base_url: &str, user_agent: Option<&str>, direct: bool) -> Result<Client, BridgeError> {
    let base_url = reqwest::Url::parse(base_url).map_err(|e| BridgeError::Network(e.to_string()))?;
    check_base_url(&base_url)?;
    // A dead session may answer with a redirect to the login page instead of a 401. Redirects inside the service are
    // followed, one that leaves it is not (the 3xx answer then counts as "session rejected", see `Client::body`),
    // so the login page's HTML is never mistaken for data and the cookies never travel to a foreign host.
    let home = base_url.host_str().unwrap_or_default().to_string();
    let redirects = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.url().host_str() == Some(home.as_str()) && attempt.previous().len() < 5 { attempt.follow() } else { attempt.stop() }
    });
    let jar = Arc::new(Jar::default());
    let mut builder = reqwest::Client::builder().cookie_provider(jar.clone()).redirect(redirects).connect_timeout(CONNECT_TIMEOUT).timeout(REQUEST_TIMEOUT);
    if direct {
        // Ignore the proxy settings of the system and of the environment: a foreign proxy makes Gosuslugi refuse everything.
        builder = builder.no_proxy();
    }
    // An empty string means "send no User-Agent header at all".
    match user_agent.unwrap_or(USER_AGENT) {
        "" => {}
        ua => builder = builder.user_agent(ua),
    }
    let http = builder
        .build()
        .map_err(|e| BridgeError::Network(e.to_string()))?;
    Ok(Client { http, jar, base_url })
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    #[uniffi::constructor]
    pub fn new() -> Result<Self, BridgeError> {
        build_client(DEFAULT_BASE_URL, None, false)
    }

    /// Same as `new`, but against another host (mock server in tests).
    #[uniffi::constructor]
    pub fn with_base_url(base_url: String) -> Result<Self, BridgeError> {
        build_client(&base_url, None, false)
    }

    /// Override the `User-Agent` header (`""` sends none). For diagnosing server-side filtering.
    #[uniffi::constructor]
    pub fn with_user_agent(user_agent: String) -> Result<Self, BridgeError> {
        build_client(DEFAULT_BASE_URL, Some(&user_agent), false)
    }

    /// Like `new`, but connects straight to the server and ignores any proxy configured in the system or the environment.
    #[uniffi::constructor]
    pub fn direct() -> Result<Self, BridgeError> {
        build_client(DEFAULT_BASE_URL, None, true)
    }

    /// `with_base_url` that ignores proxies (for tests).
    #[uniffi::constructor]
    pub fn with_base_url_direct(base_url: String) -> Result<Self, BridgeError> {
        build_client(&base_url, None, true)
    }

    /// Whether the server can be reached from here, and what it says to an anonymous request: the HTTP status
    /// (401 is the normal answer without a session). A transport failure is an error. Used to find out if a proxy
    /// in between breaks the connection (Gosuslugi refuses foreign addresses).
    pub async fn probe(&self) -> Result<u16, BridgeError> {
        let mut url = self.base_url.join("/api/myschool/v2/auth/student").map_err(|e| BridgeError::Network(e.to_string()))?;
        url.query_pairs_mut().append_pair("role", "student");
        let resp = self.http.get(url).header(reqwest::header::ACCEPT, "application/json").send().await.map_err(|e| BridgeError::Network(e.to_string()))?;
        Ok(resp.status().as_u16())
    }

    /// Two-letter country code (upper case) of the address this client's traffic leaves from, so a proxy abroad can be told from
    /// a local one. Asks public "what is my country" services, one after another, through the client's own route (its proxy included).
    pub async fn exit_country(&self) -> Result<String, BridgeError> {
        self.exit_country_from(GEO_SERVICES.iter().map(|s| s.to_string()).collect()).await
    }

    /// `exit_country` against the given service URLs (they answer JSON with a `country` field, or the bare code as text).
    pub async fn exit_country_from(&self, services: Vec<String>) -> Result<String, BridgeError> {
        let mut last = BridgeError::Network("no geo service".into());
        for url in services {
            let attempt = async {
                let resp = self.http.get(&url).send().await.map_err(|e| BridgeError::Network(e.to_string()))?;
                if !resp.status().is_success() {
                    return Err(BridgeError::Network(format!("HTTP {}", resp.status())));
                }
                let text = resp.text().await.map_err(|e| BridgeError::Network(e.to_string()))?;
                parse_country(&text).ok_or_else(|| BridgeError::Parse("no country in the answer".into()))
            };
            match attempt.await {
                Ok(code) => return Ok(code),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Install the session obtained by the UI (WebView on mobile, browser on desktop).
    pub fn set_session(&self, cookies: Vec<SessionCookie>) {
        for c in cookies.into_iter().filter(plain_cookie) {
            self.jar.add_cookie_str(&format!("{}={}; Path=/", c.name, c.value), &self.base_url);
        }
    }

    /// Same as `set_session`, from a raw `Cookie` header string. Returns how many cookies were set.
    pub fn set_session_from_header(&self, header: String) -> u32 {
        let cookies: Vec<SessionCookie> = parse_cookie_header(&header).into_iter().filter(plain_cookie).collect();
        let n = cookies.len() as u32;
        self.set_session(cookies);
        n
    }

    /// `true` if the installed session is accepted by the server, `false` if it is missing or
    /// expired (the UI should then run the login flow again). Other failures are errors.
    pub async fn check_session(&self) -> Result<bool, BridgeError> {
        match self.students().await {
            // The server accepted the session. An account without a linked student is still a valid session: the caller
            // should say "no student", not send the user round the login again.
            Ok(_) => Ok(true),
            Err(BridgeError::Auth(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Students linked to the logged-in account; the first one is normally the current user.
    pub async fn students(&self) -> Result<Vec<Student>, BridgeError> {
        self.students_for_role("student".to_string()).await
    }

    /// Same as `students`, for another account role (the site sends `role=student`; a parent
    /// account probably needs a different value, which has not been captured yet).
    pub async fn students_for_role(&self, role: String) -> Result<Vec<Student>, BridgeError> {
        let mut url = self
            .base_url
            .join("/api/myschool/v2/auth/student")
            .map_err(|e| BridgeError::Network(e.to_string()))?;
        url.query_pairs_mut().append_pair("role", &role);
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| BridgeError::Network(e.to_string()))?;
        datamart::parse_students(&Self::body(resp).await?)
    }

    /// Lessons, homework and marks for one ISO week.
    pub async fn week(&self, student_id: String, year: u32, iso_week: u32) -> Result<Week, BridgeError> {
        check_week(year, iso_week)?;
        let body = self.datamart(&datamart::week_queries(&student_id, year, iso_week)).await?;
        datamart::parse_week(&body, year, iso_week)
    }

    /// Class, school and quarter dates for the academic year.
    pub async fn class_info(&self, student_id: String, year: u32) -> Result<Option<ClassInfo>, BridgeError> {
        check_year(year)?;
        let body = self.datamart(&datamart::class_queries(&student_id, year)).await?;
        datamart::parse_class(&body)
    }
}

impl Client {
    async fn body(resp: reqwest::Response) -> Result<String, BridgeError> {
        let status = resp.status();
        // 401/403, or a redirect that was not followed (to the login page): the session is not accepted.
        // (A 403 can in principle also come from a firewall; the status stays in the message for diagnosis.)
        if status == 401 || status == 403 || status.is_redirection() {
            return Err(BridgeError::Auth(format!("HTTP {status}")));
        }
        if !status.is_success() {
            // Servers put the reason in the body; keep a short prefix for diagnostics.
            let detail: String = resp.text().await.unwrap_or_default().chars().take(200).collect();
            return Err(BridgeError::Network(format!("HTTP {status}: {detail}")));
        }
        resp.text().await.map_err(|e| BridgeError::Network(e.to_string()))
    }

    async fn datamart(&self, queries: &[Query<'_>]) -> Result<String, BridgeError> {
        let url = self
            .base_url
            .join("/api/myschool/v1/datamart")
            .map_err(|e| BridgeError::Network(e.to_string()))?;
        let resp = self
            .http
            .post(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(queries)
            .send()
            .await
            .map_err(|e| BridgeError::Network(e.to_string()))?;
        Self::body(resp).await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn user_agent_names_the_product_version_and_os_family_only() {
        let os = if cfg!(target_os = "linux") { "Linux" } else if cfg!(target_os = "windows") { "Windows" } else if cfg!(target_os = "android") { "Android" } else { "" };
        if !os.is_empty() {
            assert_eq!(super::USER_AGENT, format!("OpenSchool/{} ({os}; +https://github.com/Xomel45/OpenSchool-Bridge)", env!("CARGO_PKG_VERSION")));
        }
        assert!(!super::USER_AGENT.contains("Mozilla"));
    }

    use super::*;

    #[test]
    fn session_cookies_only_go_to_https_or_a_local_test_server() {
        assert!(Client::with_base_url("https://www.gosuslugi.ru".into()).is_ok());
        assert!(Client::with_base_url("http://127.0.0.1:8080".into()).is_ok());
        assert!(Client::with_base_url("http://localhost:1".into()).is_ok());
        assert!(Client::with_base_url("http://evil.example".into()).is_err(), "plain http to a remote host");
        assert!(Client::with_base_url("http://127.0.0.1.evil.example".into()).is_err(), "look-alike host");
        assert!(Client::with_base_url("ftp://example.com".into()).is_err());
        assert!(Client::with_base_url("not a url".into()).is_err());
    }

    #[tokio::test]
    async fn a_server_that_never_answers_ends_in_an_error_not_a_hang() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _held = std::thread::spawn(move || listener.accept().map(|(s, _)| { std::thread::sleep(std::time::Duration::from_secs(5)); s }));
        let client = Client::with_base_url(format!("http://127.0.0.1:{port}")).unwrap();
        let started = std::time::Instant::now();
        let r = client.students().await;
        assert!(matches!(r, Err(BridgeError::Network(_))), "{r:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "took {:?}", started.elapsed());
    }

    /// A one-shot HTTP server that answers every connection with `status` and `body`.
    fn serve(status: &str, body: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let status = status.to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn check_session_tells_a_rejected_session_from_an_account_without_students() {
        let rejected = Client::with_base_url(serve("401 Unauthorized", "")).unwrap();
        assert!(!rejected.check_session().await.unwrap(), "401: not logged in");
        let no_students = Client::with_base_url(serve("200 OK", "[]")).unwrap();
        assert!(no_students.check_session().await.unwrap(), "the server accepted the session even though no student is linked");
        let broken = Client::with_base_url(serve("500 Internal Server Error", "oops")).unwrap();
        assert!(broken.check_session().await.is_err(), "a server error is an error, not an answer about the session");
    }

    #[test]
    fn country_answers_in_both_shapes_and_nothing_else_passes() {
        assert_eq!(parse_country(r#"{"ip":"1.2.3.4","country":"de"}"#).as_deref(), Some("DE"));
        assert_eq!(parse_country("RU\n").as_deref(), Some("RU"));
        for bad in ["", "Russia", "<html>blocked</html>", r#"{"country":"RUS"}"#, r#"{"ip":"1"}"#, "R1"] {
            assert_eq!(parse_country(bad), None, "{bad:?}");
        }
    }

    #[tokio::test]
    async fn exit_country_skips_a_broken_service_and_fails_when_all_are_broken() {
        let c = Client::with_base_url("http://127.0.0.1:1".into()).unwrap();
        let got = c.exit_country_from(vec!["http://127.0.0.1:1/".into(), serve("500 Oops", "x"), serve("200 OK", "junk text"), serve("200 OK", r#"{"country":"NL"}"#)]).await;
        assert_eq!(got.unwrap(), "NL");
        assert!(c.exit_country_from(vec![serve("200 OK", "junk text"), "http://127.0.0.1:1/".into()]).await.is_err());
        assert!(c.exit_country_from(vec![]).await.is_err());
    }

    #[tokio::test]
    async fn probe_reports_the_status_and_fails_on_a_dead_port() {
        assert_eq!(Client::with_base_url(serve("401 Unauthorized", "")).unwrap().probe().await.unwrap(), 401);
        assert_eq!(Client::with_base_url(serve("403 Forbidden", "")).unwrap().probe().await.unwrap(), 403);
        assert!(Client::with_base_url("http://127.0.0.1:1".into()).unwrap().probe().await.is_err());
    }

    #[tokio::test]
    async fn impossible_weeks_are_refused_before_any_request() {
        let client = Client::with_base_url("http://127.0.0.1:1".into()).unwrap(); // nothing listens there: a request would be a Network error
        for (year, week) in [(2026, 0), (2026, 54), (2026, 99), (1999, 10), (2101, 10)] {
            assert!(matches!(client.week("s".into(), year, week).await, Err(BridgeError::Parse(_))), "{year} week {week}");
        }
        assert!(matches!(client.week("s".into(), 2026, 53).await, Err(BridgeError::Network(_))), "2026 has 53 weeks, so the request is attempted");
        assert!(matches!(client.week("s".into(), 2025, 53).await, Err(BridgeError::Parse(_))), "2025 has only 52");
        assert!(matches!(client.class_info("s".into(), 1999).await, Err(BridgeError::Parse(_))));
        assert!(matches!(client.class_info("s".into(), 2026).await, Err(BridgeError::Network(_))));
    }

    #[test]
    fn iso_week_counts_match_the_calendar() {
        // known 53-week years: 2015, 2020, 2026; 52-week: 2021-2025, 2027
        assert_eq!([2015, 2020, 2026].map(iso_weeks_in), [53, 53, 53]);
        assert_eq!([2021, 2022, 2023, 2024, 2025, 2027].map(iso_weeks_in), [52; 6]);
    }

    #[test]
    fn cookies_that_would_break_out_of_their_value_are_not_sent() {
        let c = Client::new().unwrap();
        assert_eq!(c.set_session_from_header("a=1; b=x\r\nInjected: 1; ok=2".into()), 2);
        let bad = |n: &str, v: &str| plain_cookie(&SessionCookie { name: n.into(), value: v.into() });
        assert!(bad("a", "b"));
        assert!(!bad("a", "b; Domain=evil.example"));
        assert!(!bad("", "b"));
        assert!(!bad("a\nb", "c"));
    }

    #[tokio::test]
    async fn a_redirect_to_the_login_page_counts_as_a_rejected_session() {
        // the "service" answers every request with a 302 to another host (the ESIA login)
        let redirected = Client::with_base_url(serve("302 Found\r\nLocation: http://login.invalid/auth", "")).unwrap();
        assert!(!redirected.check_session().await.unwrap(), "a redirect off the service means: log in again, not a Parse error");
        assert!(matches!(redirected.students().await, Err(BridgeError::Auth(_))));
    }

    #[test]
    fn cookie_header_is_split_and_trimmed() {
        let c = parse_cookie_header(" a=1; b = two=2 ;broken; =x;c=");
        let pairs: Vec<_> = c.iter().map(|c| (c.name.as_str(), c.value.as_str())).collect();
        assert_eq!(pairs, vec![("a", "1"), ("b", "two=2"), ("c", "")]);
    }
}
