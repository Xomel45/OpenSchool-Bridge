//! Runs in its own process because the app version is set once per process.
use std::io::{Read, Write};
use std::sync::mpsc;

use openschool_bridge::{set_app_version, Client};

/// A server that answers 401 and hands over the first request it got.
fn serve() -> (u16, mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = write!(stream, "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    (port, rx)
}

#[tokio::test]
async fn the_user_agent_sent_carries_the_app_version_then_the_bridge_version() {
    assert!(!set_app_version("1.0\r\nX-Evil: 1".into()), "a version with a line break is refused");
    assert!(set_app_version("7.7.7".into()));
    assert!(!set_app_version("8.8.8".into()), "only the first call counts");

    let (port, requests) = serve();
    let client = Client::with_base_url(format!("http://127.0.0.1:{port}")).unwrap();
    assert_eq!(client.probe().await.unwrap(), 401);
    let request = requests.recv().unwrap().to_ascii_lowercase();
    let ua = request.lines().find_map(|l| l.strip_prefix("user-agent: ")).expect("a User-Agent header").to_string();
    assert!(ua.starts_with("openschool/7.7.7 ("), "{ua}");
    assert!(ua.contains(&format!("; bridge {}; ", env!("CARGO_PKG_VERSION"))), "{ua}");
    assert!(!request.contains("x-evil"));
}
