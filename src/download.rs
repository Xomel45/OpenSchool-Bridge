//! Downloading a homework attachment (the `link` of a `Material`).
//!
//! The server names the file and the address, so everything about it is checked here: the link must be https (plain http only for
//! the test server on this machine), redirects are followed by hand and every hop is checked again, the session cookies are added
//! only for Gosuslugi hosts (and never follow a redirect to another site), the size is capped while reading, and a web page
//! (a login or an error page) is not taken for a file. The file name is cleaned so it cannot point outside a folder.

use reqwest::header::{ACCEPT, CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, LOCATION};
use reqwest::Url;

use crate::client::Client;
use crate::error::BridgeError;

/// The default and the largest file taken in one go.
pub const MAX_DOWNLOAD: u64 = 50 * 1024 * 1024;
/// A whole download (connect, redirects, reading) must end by then.
pub const DOWNLOAD_TIMEOUT: std::time::Duration = if cfg!(test) { std::time::Duration::from_secs(3) } else { std::time::Duration::from_secs(180) };
const MAX_REDIRECTS: usize = 5;

/// A downloaded file. `file_name` is the server's name for it, already cleaned (it may be empty of an extension).
#[derive(Debug, Clone, uniffi::Record)]
pub struct Download {
    pub file_name: String,
    /// The media type the server sent, lower case, without parameters (may be empty).
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// A name that is safe to create in a folder: no path, no characters Windows or Linux shells dislike, no reserved device names.
#[uniffi::export]
pub fn safe_file_name(name: String) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    if cleaned.is_empty() {
        return "file".into();
    }
    // keep the extension when the name has to be cut
    let (stem, ext) = match cleaned.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.chars().count() <= 10 => (s.to_string(), format!(".{e}")),
        _ => (cleaned.clone(), String::new()),
    };
    let stem: String = stem.chars().take(100).collect();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"];
    let stem = if reserved.contains(&stem.to_ascii_uppercase().as_str()) { format!("_{stem}") } else { stem };
    format!("{stem}{ext}")
}

/// The name a downloaded attachment gets: what the page shows for it (cleaned), with the server's extension when the shown name has none.
/// Both apps use this one rule.
#[uniffi::export]
pub fn attachment_file_name(shown: String, server: String) -> String {
    if shown.trim().is_empty() {
        return safe_file_name(server);
    }
    let shown = safe_file_name(shown);
    let ext_of = |n: &str| n.rsplit_once('.').filter(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric())).map(|(_, e)| e.to_string());
    match (ext_of(&shown), ext_of(&server)) {
        (None, Some(ext)) => format!("{shown}.{ext}"),
        _ => shown,
    }
}

/// Kinds of files that are opened by the system's default program after the download (documents, pictures, sound, video).
/// Anything else (programs, scripts, archives, unknown) is only saved.
#[uniffi::export]
pub fn can_open(file_name: String) -> bool {
    const OPENABLE: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "txt", "csv", "png", "jpg", "jpeg", "gif", "webp", "bmp", "mp3", "wav", "ogg", "mp4", "webm"];
    file_name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && OPENABLE.contains(&ext.to_ascii_lowercase().as_str()))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The file name of a `Content-Disposition` header (`filename*=UTF-8''…` wins over `filename="…"`).
fn disposition_name(header: &str) -> Option<String> {
    let mut plain = None;
    for part in header.split(';').skip(1) {
        let (key, value) = part.trim().split_once('=')?;
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                let encoded = value.split_once("''").map(|(_, v)| v).unwrap_or(value);
                return Some(percent_decode(encoded.trim_matches('"')));
            }
            "filename" => plain = Some(value.trim_matches('"').to_string()),
            _ => {}
        }
    }
    plain
}

impl Client {
    /// Whether the session cookies may be sent to this address: the host of the service itself, and the Gosuslugi hosts below it
    /// (the files live on another Gosuslugi host), https only (plain http only for the service itself on a test server).
    pub(crate) fn cookies_allowed_for(&self, url: &Url) -> bool {
        let (Some(host), Some(base)) = (url.host_str(), self.base_url.host_str()) else { return false };
        if host == base && url.scheme() == self.base_url.scheme() && url.port_or_known_default() == self.base_url.port_or_known_default() {
            return true;
        }
        url.scheme() == "https" && base.ends_with("gosuslugi.ru") && (host == "gosuslugi.ru" || host.ends_with(".gosuslugi.ru"))
    }

    /// A download may go to any https address the server sends us to (files are often on a storage host), but never to plain http
    /// (except the test server), and never with a user name in the address.
    fn check_download_url(&self, url: &Url) -> Result<(), BridgeError> {
        if !url.username().is_empty() || url.password().is_some() {
            return Err(BridgeError::Parse("link with a user name".into()));
        }
        let same_local = url.scheme() == "http" && url.host_str() == self.base_url.host_str() && self.base_url.scheme() == "http";
        if url.scheme() == "https" || same_local { Ok(()) } else { Err(BridgeError::Parse("link is not https".into())) }
    }

    fn cookie_header(&self) -> Option<String> {
        let kept = self.cookies.lock().unwrap_or_else(|p| p.into_inner());
        (!kept.is_empty()).then(|| kept.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; "))
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    /// Download the file a homework `link` points to (an address relative to the service is also fine). At most `max_bytes`
    /// (and never more than [`MAX_DOWNLOAD`]; 0 means the largest). Too large -> `Parse("file too big")`; a web page instead of a
    /// file -> `Parse("not a file")`; 401/403 from the service itself -> `Auth`.
    pub async fn download(&self, link: String, max_bytes: u64) -> Result<Download, BridgeError> {
        let limit = if max_bytes == 0 { MAX_DOWNLOAD } else { max_bytes.min(MAX_DOWNLOAD) };
        let mut url = self.base_url.join(link.trim()).map_err(|_| BridgeError::Parse("bad link".into()))?;
        for _ in 0..=MAX_REDIRECTS {
            self.check_download_url(&url)?;
            let mut request = self.dl.get(url.clone()).header(ACCEPT, "*/*");
            if self.cookies_allowed_for(&url) && let Some(cookies) = self.cookie_header() {
                request = request.header(COOKIE, cookies);
            }
            let mut resp = request.send().await.map_err(|e| BridgeError::Network(e.to_string()))?;
            let status = resp.status();
            if status.is_redirection() {
                let target = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok()).ok_or_else(|| BridgeError::Network("redirect without a target".into()))?;
                url = url.join(target).map_err(|_| BridgeError::Parse("bad redirect".into()))?;
                continue;
            }
            if status == 401 || status == 403 {
                // A refusal from the service itself means the session is gone; from a storage host it only means "not for you".
                let own = url.host_str() == self.base_url.host_str();
                return Err(if own { BridgeError::Auth(format!("HTTP {status}")) } else { BridgeError::Network(format!("HTTP {status}")) });
            }
            if !status.is_success() {
                return Err(BridgeError::Network(format!("HTTP {status}")));
            }
            if resp.content_length().is_some_and(|n| n > limit) {
                return Err(BridgeError::Parse("file too big".into()));
            }
            let mime = resp.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase()).unwrap_or_default();
            if mime == "text/html" || mime == "application/xhtml+xml" {
                return Err(BridgeError::Parse("not a file (a web page came back)".into()));
            }
            let named = resp.headers().get(CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()).and_then(disposition_name);
            let from_url = url.path_segments().and_then(|mut s| s.next_back()).map(percent_decode).unwrap_or_default();
            let file_name = safe_file_name(named.unwrap_or(from_url));
            let mut bytes: Vec<u8> = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|e| BridgeError::Network(e.to_string()))? {
                if bytes.len() as u64 + chunk.len() as u64 > limit {
                    return Err(BridgeError::Parse("file too big".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(Download { file_name, mime, bytes });
        }
        Err(BridgeError::Network("too many redirects".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    #[test]
    fn names_are_cleaned_so_they_cannot_leave_a_folder() {
        assert_eq!(safe_file_name("../../etc/passwd".into()), "passwd");
        assert_eq!(safe_file_name("C:\\Users\\x\\a.docx".into()), "a.docx");
        assert_eq!(safe_file_name("a<b>c:d?.pdf".into()), "a_b_c_d_.pdf");
        assert_eq!(safe_file_name("  ..hidden  ".into()), "hidden");
        assert_eq!(safe_file_name("".into()), "file");
        assert_eq!(safe_file_name("...".into()), "file");
        assert_eq!(safe_file_name("CON.txt".into()), "_CON.txt");
        assert_eq!(safe_file_name("a\nb\u{0}c.txt".into()), "a_b_c.txt");
        let long = format!("{}.pdf", "я".repeat(300));
        let cut = safe_file_name(long);
        assert!(cut.ends_with(".pdf") && cut.chars().count() <= 104, "{cut}");
    }

    #[test]
    fn the_shown_name_wins_and_borrows_the_servers_extension() {
        let f = |a: &str, b: &str| attachment_file_name(a.into(), b.into());
        assert_eq!(f("Задание 5", "x.docx"), "Задание 5.docx");
        assert_eq!(f("Задание.pdf", "x.docx"), "Задание.pdf");
        assert_eq!(f("", "server.pdf"), "server.pdf");
        assert_eq!(f("../../evil", "a.pdf"), "evil.pdf");
        assert_eq!(f("noext", "file"), "noext");
    }

    #[test]
    fn only_documents_pictures_and_media_are_opened() {
        for ok in ["a.pdf", "A.DOCX", "x.jpeg", "v.mp4"] {
            assert!(can_open(ok.into()), "{ok}");
        }
        for no in ["a.exe", "a.sh", "a.desktop", "a.zip", "a.apk", "a.html", "a.svg", "a", ".pdf", "a.pdf.exe", "a.js", "a.bat", "a.lnk"] {
            assert!(!can_open(no.into()), "{no}");
        }
    }

    #[test]
    fn a_broken_percent_sequence_is_kept_and_never_panics() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode("%é9"), "%é9");
        assert_eq!(percent_decode("a%20b"), "a b");
    }

    #[test]
    fn the_disposition_header_gives_the_name() {
        assert_eq!(disposition_name("attachment; filename=\"a b.docx\"").as_deref(), Some("a b.docx"));
        assert_eq!(disposition_name("attachment; filename=\"x.txt\"; filename*=UTF-8''%D0%94%D0%97.docx").as_deref(), Some("ДЗ.docx"));
        assert_eq!(disposition_name("inline"), None);
    }

    #[test]
    fn cookies_go_to_gosuslugi_hosts_over_https_only() {
        let c = Client::new().unwrap();
        let ok = |u: &str| c.cookies_allowed_for(&Url::parse(u).unwrap());
        assert!(ok("https://www.gosuslugi.ru/x") && ok("https://edu.gosuslugi.ru/storage/a") && ok("https://gosuslugi.ru/"));
        for bad in ["https://evil.example/", "https://gosuslugi.ru.evil.example/", "https://edu.gosuslugi.ru.evil.example/", "https://notgosuslugi.ru/", "http://edu.gosuslugi.ru/a", "https://storage.yandexcloud.net/a"] {
            assert!(!ok(bad), "{bad}");
        }
    }

    #[test]
    fn links_must_be_https_and_without_a_user_name() {
        let c = Client::new().unwrap();
        let bad = |u: &str| c.check_download_url(&Url::parse(u).unwrap()).is_err();
        assert!(!bad("https://edu.gosuslugi.ru/storage/a"));
        assert!(bad("http://edu.gosuslugi.ru/a") && bad("ftp://x/a") && bad("file:///etc/passwd") && bad("https://user:pw@edu.gosuslugi.ru/a") && bad("javascript:alert(1)"));
    }

    /// A server: `route(path) -> raw response`, remembers the request heads.
    fn serve(route: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                log.lock().unwrap().push(head);
                let _ = stream.write_all(&route(&path));
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    fn ok(content_type: &str, extra: &str, body: &[u8]) -> Vec<u8> {
        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        r.extend_from_slice(body);
        r
    }

    #[tokio::test]
    async fn a_file_comes_with_its_name_and_the_session_cookies_of_the_service() {
        let (base, seen) = serve(|path| match path {
            "/start" => b"HTTP/1.1 302 Found\r\nLocation: /storage/real?x=1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            _ => ok("application/pdf; charset=binary", "Content-Disposition: attachment; filename*=UTF-8''%D0%94%D0%97.pdf\r\n", b"%PDF-1.4 hello"),
        });
        let c = Client::with_base_url_direct(base).unwrap();
        c.set_session_from_header("sid=abc; other=1".into());
        let d = c.download("/start".into(), 0).await.unwrap();
        assert_eq!((d.file_name.as_str(), d.mime.as_str(), d.bytes.as_slice()), ("ДЗ.pdf", "application/pdf", &b"%PDF-1.4 hello"[..]));
        let heads = seen.lock().unwrap();
        assert_eq!(heads.len(), 2, "one redirect was followed");
        assert!(heads.iter().all(|h| h.contains("sid=abc") && h.contains("other=1")), "{heads:?}");
    }

    #[tokio::test]
    async fn the_name_falls_back_to_the_end_of_the_address_and_is_cleaned() {
        let (base, _) = serve(|_| ok("application/octet-stream", "", b"x"));
        let c = Client::with_base_url_direct(base).unwrap();
        assert_eq!(c.download("/storage/%2E%2E%2Fevil%5Cname.docx".into(), 0).await.unwrap().file_name, "name.docx");
    }

    #[tokio::test]
    async fn a_file_over_the_limit_is_refused_by_its_announced_size_and_by_what_actually_arrives() {
        let (base, _) = serve(|path| if path == "/big" { ok("application/pdf", "", &[1u8; 100]) } else { b"HTTP/1.1 200 OK\r\nContent-Type: application/pdf\r\nConnection: close\r\n\r\n0123456789012345678901234567890123456789".to_vec() });
        let c = Client::with_base_url_direct(base).unwrap();
        assert!(matches!(c.download("/big".into(), 50).await, Err(BridgeError::Parse(m)) if m == "file too big"));
        assert!(matches!(c.download("/unannounced".into(), 20).await, Err(BridgeError::Parse(m)) if m == "file too big"), "no Content-Length: counted while reading");
        assert_eq!(c.download("/big".into(), 100).await.unwrap().bytes.len(), 100);
    }

    #[tokio::test]
    async fn a_web_page_is_not_a_file_and_a_refusal_of_the_service_means_log_in_again() {
        let (base, _) = serve(|path| if path == "/login" { ok("text/html; charset=utf-8", "", b"<html>log in</html>") } else { b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec() });
        let c = Client::with_base_url_direct(base).unwrap();
        assert!(matches!(c.download("/login".into(), 0).await, Err(BridgeError::Parse(m)) if m.starts_with("not a file")));
        assert!(matches!(c.download("/forbidden".into(), 0).await, Err(BridgeError::Auth(_))));
    }

    #[tokio::test]
    async fn redirects_are_limited_and_cannot_leave_https() {
        let (base, _) = serve(|_| b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
        let c = Client::with_base_url_direct(base).unwrap();
        assert!(matches!(c.download("/loop".into(), 0).await, Err(BridgeError::Network(m)) if m.contains("too many")));
        let (base2, _) = serve(|_| b"HTTP/1.1 302 Found\r\nLocation: http://evil.example/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
        let c2 = Client::with_base_url_direct(base2).unwrap();
        assert!(matches!(c2.download("/x".into(), 0).await, Err(BridgeError::Parse(m)) if m.contains("not https")));
    }
}
