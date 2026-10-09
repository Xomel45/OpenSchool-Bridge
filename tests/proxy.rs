//! Runs in its own process because it sets proxy environment variables.
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use openschool_bridge::Client;

/// A server answering every request with `status`; counts what it received.
fn serve(status: &'static str, hits: Arc<AtomicUsize>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            hits.fetch_add(1, Ordering::SeqCst);
            let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    port
}

#[tokio::test]
async fn direct_client_skips_the_proxy_and_the_normal_one_uses_it() {
    let (server_hits, proxy_hits) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let server = serve("401 Unauthorized", server_hits.clone());
    let proxy = serve("403 Forbidden", proxy_hits.clone()); // the "foreign proxy" refuses
    // SAFETY: this test binary has a single test, so nothing reads the environment concurrently.
    unsafe {
        std::env::set_var("HTTP_PROXY", format!("http://127.0.0.1:{proxy}"));
        std::env::set_var("http_proxy", format!("http://127.0.0.1:{proxy}"));
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
    }
    let base = format!("http://127.0.0.1:{server}");

    let through_proxy = Client::with_base_url(base.clone()).unwrap();
    assert_eq!(through_proxy.probe().await.unwrap(), 403, "the normal client goes through the proxy");
    assert_eq!((server_hits.load(Ordering::SeqCst), proxy_hits.load(Ordering::SeqCst)), (0, 1));

    let direct = Client::with_base_url_direct(base).unwrap();
    assert_eq!(direct.probe().await.unwrap(), 401, "the direct client reaches the server itself");
    assert_eq!((server_hits.load(Ordering::SeqCst), proxy_hits.load(Ordering::SeqCst)), (1, 1));
}
