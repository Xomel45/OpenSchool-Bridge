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

/// ISO weeks run 1..=53.
fn check_week(year: u32, iso_week: u32) -> Result<(), BridgeError> {
    check_year(year)?;
    if (1..=53).contains(&iso_week) { Ok(()) } else { Err(BridgeError::Parse(format!("invalid week {iso_week} of {year}"))) }
}

fn build_client(base_url: &str, user_agent: Option<&str>) -> Result<Client, BridgeError> {
    let base_url = reqwest::Url::parse(base_url).map_err(|e| BridgeError::Network(e.to_string()))?;
    check_base_url(&base_url)?;
    let jar = Arc::new(Jar::default());
    let mut builder = reqwest::Client::builder().cookie_provider(jar.clone()).connect_timeout(CONNECT_TIMEOUT).timeout(REQUEST_TIMEOUT);
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
        build_client(DEFAULT_BASE_URL, None)
    }

    /// Same as `new`, but against another host (mock server in tests).
    #[uniffi::constructor]
    pub fn with_base_url(base_url: String) -> Result<Self, BridgeError> {
        build_client(&base_url, None)
    }

    /// Override the `User-Agent` header (`""` sends none). For diagnosing server-side filtering.
    #[uniffi::constructor]
    pub fn with_user_agent(user_agent: String) -> Result<Self, BridgeError> {
        build_client(DEFAULT_BASE_URL, Some(&user_agent))
    }

    /// Install the session obtained by the UI (WebView on mobile, browser on desktop).
    pub fn set_session(&self, cookies: Vec<SessionCookie>) {
        for c in cookies {
            self.jar.add_cookie_str(&format!("{}={}; Path=/", c.name, c.value), &self.base_url);
        }
    }

    /// Same as `set_session`, from a raw `Cookie` header string. Returns how many cookies were set.
    pub fn set_session_from_header(&self, header: String) -> u32 {
        let cookies = parse_cookie_header(&header);
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
        if status == 401 || status == 403 {
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

    #[tokio::test]
    async fn impossible_weeks_are_refused_before_any_request() {
        let client = Client::with_base_url("http://127.0.0.1:1".into()).unwrap(); // nothing listens there: a request would be a Network error
        for (year, week) in [(2026, 0), (2026, 54), (2026, 99), (1999, 10), (2101, 10)] {
            assert!(matches!(client.week("s".into(), year, week).await, Err(BridgeError::Parse(_))), "{year} week {week}");
        }
        assert!(matches!(client.week("s".into(), 2026, 53).await, Err(BridgeError::Network(_))), "week 53 is valid, so the request is attempted");
        assert!(matches!(client.class_info("s".into(), 1999).await, Err(BridgeError::Parse(_))));
        assert!(matches!(client.class_info("s".into(), 2026).await, Err(BridgeError::Network(_))));
    }

    #[test]
    fn cookie_header_is_split_and_trimmed() {
        let c = parse_cookie_header(" a=1; b = two=2 ;broken; =x;c=");
        let pairs: Vec<_> = c.iter().map(|c| (c.name.as_str(), c.value.as_str())).collect();
        assert_eq!(pairs, vec![("a", "1"), ("b", "two=2"), ("c", "")]);
    }
}
