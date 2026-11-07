use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};

/// Thin wrapper over a Unix domain socket: accept a connection, write bytes
/// to one. No pub/sub, no messaging policy; that's the caller's job.
pub struct UnixSocketServer {
    listener: UnixListener,
}

impl UnixSocketServer {
    pub fn new(socket_path: &str) -> Self {
        // clear stale socket file left behind by a previous run
        let removed = std::fs::remove_file(socket_path);
        match removed {
            Ok(_) => {}
            Err(_) => {}
        }

        // bind the socket; fatal if it fails
        let bound = UnixListener::bind(socket_path);
        let listener = match bound {
            Ok(l) => l,
            Err(e) => panic!("bind unix socket {socket_path}: {e}"),
        };

        println!("daemon: unix socket server listening at {socket_path}");

        UnixSocketServer { listener: listener }
    }

    // accepts one connection; logs and returns None on error
    pub async fn subscribe(&self) -> Option<Subscriber> {
        let accepted = self.listener.accept().await;
        match accepted {
            Ok((stream, _)) => Some(Subscriber { stream: stream }),
            Err(e) => {
                eprintln!("daemon: unix accept error: {e}");
                None
            }
        }
    }
}

pub struct Subscriber {
    stream: UnixStream,
}

impl Subscriber {
    pub async fn broadcast(&mut self, bytes: &[u8]) -> bool {
        let wrote = self.stream.write_all(bytes).await;
        match wrote {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    // short unique path per test; Linux caps socket paths at ~108 chars
    fn test_path(name: &str) -> String {
        format!("/tmp/phd-test-{}-{name}.sock", std::process::id())
    }

    #[tokio::test]
    async fn client_receives_bytes_written_by_subscriber() {
        let path = test_path("write");
        let server = UnixSocketServer::new(&path);

        let mut client = UnixStream::connect(&path).await.unwrap();
        let mut subscriber = server.subscribe().await.unwrap();

        assert!(subscriber.broadcast(b"hello\n").await);

        let mut buf = [0u8; 6];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello\n");

        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn stale_file_at_socket_path_is_replaced() {
        let path = test_path("stale");
        std::fs::write(&path, b"left over from a previous run").unwrap();

        let server = UnixSocketServer::new(&path);

        let _client = UnixStream::connect(&path).await.unwrap();
        assert!(server.subscribe().await.is_some());

        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn each_client_gets_its_own_subscriber() {
        let path = test_path("two");
        let server = UnixSocketServer::new(&path);

        let mut client_a = UnixStream::connect(&path).await.unwrap();
        let mut subscriber_a = server.subscribe().await.unwrap();
        let mut client_b = UnixStream::connect(&path).await.unwrap();
        let mut subscriber_b = server.subscribe().await.unwrap();

        assert!(subscriber_a.broadcast(b"A").await);
        assert!(subscriber_b.broadcast(b"B").await);

        let mut buf = [0u8; 1];
        client_a.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"A");
        client_b.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"B");

        std::fs::remove_file(&path).unwrap();
    }

    #[tokio::test]
    async fn broadcast_returns_false_after_client_disconnects() {
        let path = test_path("gone");
        let server = UnixSocketServer::new(&path);

        let client = UnixStream::connect(&path).await.unwrap();
        let mut subscriber = server.subscribe().await.unwrap();
        drop(client);

        // the OS may accept a write or two before reporting the broken pipe
        let mut failed = false;
        for _ in 0..100 {
            let wrote = subscriber.broadcast(b"anyone there?").await;
            if !wrote {
                failed = true;
                break;
            }
        }
        assert!(failed);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    #[should_panic(expected = "bind unix socket")]
    fn binding_in_a_missing_directory_panics() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();

        UnixSocketServer::new("/tmp/phd-no-such-dir/test.sock");
    }
}
