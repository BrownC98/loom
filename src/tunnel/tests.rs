use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

fn pair() -> (
    (Mux, mpsc::UnboundedReceiver<Stream>),
    (Mux, mpsc::UnboundedReceiver<Stream>),
) {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    (start(ar, aw), start(br, bw))
}

fn echo(mut events: mpsc::UnboundedReceiver<Stream>) {
    tokio::spawn(async move {
        while let Some(stream) = events.recv().await {
            tokio::spawn(async move {
                let (mut read, mut write) = tokio::io::split(stream);
                tokio::io::copy(&mut read, &mut write).await.unwrap();
                write.shutdown().await.unwrap();
            });
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
    let Some(mut other) = accepted.recv().await else {
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

#[tokio::test]
async fn a_stream_opened_after_the_transport_ended_is_reset_at_once() {
    let (a, b) = tokio::io::duplex(1 << 16);
    let (ar, aw) = tokio::io::split(a);
    let (opener, mut events) = start(ar, aw);
    drop(b);
    while events.recv().await.is_some() {}
    assert!(opener.is_closed());
    let mut late = opener.open();
    let mut got = Vec::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        late.read_to_end(&mut got),
    );
    assert_eq!(
        read.await.unwrap().unwrap_err().kind(),
        io::ErrorKind::ConnectionReset
    );
}
