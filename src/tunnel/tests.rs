#[cfg(unix)]
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

fn pair() -> (
    (Mux, mpsc::UnboundedReceiver<Event>),
    (Mux, mpsc::UnboundedReceiver<Event>),
) {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    (start(ar, aw), start(br, bw))
}

fn echo(mut events: mpsc::UnboundedReceiver<Event>) {
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if let Event::Opened(stream) = event {
                tokio::spawn(async move {
                    let (mut read, mut write) = tokio::io::split(stream);
                    tokio::io::copy(&mut read, &mut write).await.unwrap();
                    write.shutdown().await.unwrap();
                });
            }
        }
    });
}

fn body(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31) ^ seed)
        .collect()
}

#[tokio::test]
async fn parallel_streams_carry_large_bodies_both_ways() {
    let ((opener, _), (_, accepted)) = pair();
    echo(accepted);
    let mut all = tokio::task::JoinSet::new();
    for seed in 0..16u8 {
        let stream = opener.open();
        all.spawn(async move {
            let sent = body(seed, 3 << 20);
            let (mut read, mut write) = tokio::io::split(stream);
            let writer = {
                let sent = sent.clone();
                tokio::spawn(async move {
                    write.write_all(&sent).await.unwrap();
                    write.shutdown().await.unwrap();
                })
            };
            let mut back = Vec::new();
            read.read_to_end(&mut back).await.unwrap();
            writer.await.unwrap();
            assert!(back == sent, "stream {seed}");
        });
    }
    while let Some(done) = all.join_next().await {
        done.unwrap();
    }
}

#[tokio::test]
async fn a_dropped_stream_ends_the_reads_and_writes_of_the_other_side() {
    let ((opener, _), (_, mut accepted)) = pair();
    let mut stream = opener.open();
    stream.write_all(b"ping").await.unwrap();
    let Some(Event::Opened(mut other)) = accepted.recv().await else {
        panic!("no stream");
    };
    let mut ping = [0u8; 4];
    other.read_exact(&mut ping).await.unwrap();
    assert_eq!(&ping, b"ping");
    other.write_all(b"last").await.unwrap();
    drop(other);
    let mut got = Vec::new();
    let err = stream.read_to_end(&mut got).await.unwrap_err();
    assert_eq!(got, b"last");
    assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
    let err = stream.write_all(b"more").await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
}

#[tokio::test]
async fn an_ended_transport_resets_every_stream_at_once() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (ar, aw) = tokio::io::split(a);
    let (opener, _events) = start(ar, aw);
    let mut streams: Vec<_> = (0..5).map(|_| opener.open()).collect();
    drop(b);
    for stream in &mut streams {
        let mut got = Vec::new();
        assert!(stream.read_to_end(&mut got).await.is_err());
    }
}

#[cfg(unix)]
async fn ask(sock: &std::path::Path, request: String) -> String {
    let mut client = tokio::net::UnixStream::connect(sock).await.unwrap();
    client.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    client.read_to_end(&mut answer).await.unwrap();
    String::from_utf8_lossy(&answer).into_owned()
}

#[cfg(unix)]
fn post(path: &str, body: &[u8]) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nhost: 127.0.0.1\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        String::from_utf8_lossy(body)
    )
}

#[cfg(unix)]
#[tokio::test]
async fn the_router_answers_through_the_relay_while_a_long_wait_stays_open() {
    use axum::routing::post as route;
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("run/daemon.sock");
    let (to_relay, relay_in) = tokio::io::duplex(1 << 16);
    let (relay_out, from_relay) = tokio::io::duplex(1 << 16);
    let relay = tokio::spawn({
        let sock = sock.clone();
        async move { super::relay(&sock, relay_in, relay_out).await }
    });
    let (doors, door) = Door::new();
    let mut events = accept(from_relay, to_relay);
    let Some(Event::Ready) = events.recv().await else {
        panic!("no ready frame");
    };
    tokio::spawn(async move {
        while let Some(Event::Opened(stream)) = events.recv().await {
            doors.send(stream).unwrap();
        }
    });
    let open = Arc::new(tokio::sync::Notify::new());
    let router = axum::Router::new()
        .route("/echo", route(|body: Bytes| async move { body }))
        .route(
            "/wait",
            route({
                let open = open.clone();
                || async move {
                    open.notified().await;
                    "mail"
                }
            }),
        );
    tokio::spawn(async move { axum::serve(door, router).await });
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(sock.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);

    let waiting = tokio::spawn(ask_owned(sock.clone(), post("/wait", b"")));
    let big = body(7, 1536 * 1024)
        .into_iter()
        .map(|b| b'a' + b % 26)
        .collect::<Vec<_>>();
    let answer = ask(&sock, post("/echo", &big)).await;
    assert!(answer.starts_with("HTTP/1.1 200 OK"), "{}", &answer[..80]);
    assert!(
        answer.ends_with(&*String::from_utf8_lossy(&big)),
        "{} {}",
        answer.len(),
        &answer[..answer.len().min(300)]
    );
    assert!(!waiting.is_finished());
    open.notify_one();
    assert!(waiting.await.unwrap().ends_with("mail"));
    relay.abort();
}

#[cfg(unix)]
async fn ask_owned(sock: std::path::PathBuf, request: String) -> String {
    ask(&sock, request).await
}

#[cfg(unix)]
#[tokio::test]
async fn the_relay_ends_on_stdin_eof_and_closes_its_clients() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("run/daemon.sock");
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    std::fs::write(&sock, "stale").unwrap();
    let (to_relay, relay_in) = tokio::io::duplex(1 << 16);
    let (relay_out, mut from_relay) = tokio::io::duplex(1 << 16);
    let relay = tokio::spawn({
        let sock = sock.clone();
        async move { super::relay(&sock, relay_in, relay_out).await }
    });
    assert_eq!(
        wire::read(&mut from_relay).await.unwrap(),
        Some(Frame::Ready)
    );
    let mut clients = Vec::new();
    for _ in 0..3 {
        clients.push(tokio::net::UnixStream::connect(&sock).await.unwrap());
        assert!(matches!(
            wire::read(&mut from_relay).await.unwrap(),
            Some(Frame::Open(_))
        ));
    }
    drop(to_relay);
    tokio::time::timeout(Duration::from_secs(5), relay)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    for mut client in clients {
        let mut got = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut got));
        assert!(matches!(read.await, Ok(Ok(0)) | Ok(Err(_))));
    }
    assert!(!sock.exists());
}
