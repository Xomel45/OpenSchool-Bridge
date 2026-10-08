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

fn build_client(base_url: &str, user_agent: Option<&str>) -> Result<Client, BridgeError> {
    let base_url = reqwest::Url::parse(base_url).map_err(|e| BridgeError::Network(e.to_string()))?;
    let jar = Arc::new(Jar::default());
    let mut builder = reqwest::Client::builder().cookie_provider(jar.clone());
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
            Ok(students) => Ok(!students.is_empty()),
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
        let body = self.datamart(&datamart::week_queries(&student_id, year, iso_week)).await?;
        datamart::parse_week(&body, year, iso_week)
    }

    /// Class, school and quarter dates for the academic year.
    pub async fn class_info(&self, student_id: String, year: u32) -> Result<Option<ClassInfo>, BridgeError> {
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
    fn cookie_header_is_split_and_trimmed() {
        let c = parse_cookie_header(" a=1; b = two=2 ;broken; =x;c=");
        let pairs: Vec<_> = c.iter().map(|c| (c.name.as_str(), c.value.as_str())).collect();
        assert_eq!(pairs, vec![("a", "1"), ("b", "two=2"), ("c", "")]);
    }
}
