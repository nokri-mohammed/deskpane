//! Both ends of a UDP link: `UdpSocketServer` receives packets (async, used
//! by the daemon) and `UdpClient` sends them to one address (blocking, used
//! by the reporter). What the bytes mean is the caller's job.

use std::net::SocketAddr;
use std::net::UdpSocket as BlockingUdpSocket;

use tokio::net::UdpSocket as AsyncUdpSocket;

/// Receiving end: bind once, then receive one packet at a time.
pub struct UdpSocketServer {
    socket: AsyncUdpSocket,
    buf: Vec<u8>,
}

impl UdpSocketServer {
    pub async fn new(addr: &str) -> Self {
        // bind the socket; fatal if it fails
        let bound = AsyncUdpSocket::bind(addr).await;
        let socket = match bound {
            Ok(s) => s,
            Err(e) => panic!("bind udp {addr}: {e}"),
        };

        println!("udp: listening on {addr}");

        UdpSocketServer {
            socket: socket,
            buf: vec![0u8; 8192],
        }
    }

    // waits for one packet; logs and returns None on error
    pub async fn receive(&mut self) -> Option<(&[u8], SocketAddr)> {
        let received = self.socket.recv_from(&mut self.buf).await;
        match received {
            Ok((len, peer)) => Some((&self.buf[..len], peer)),
            Err(e) => {
                eprintln!("udp: recv error: {e}");
                None
            }
        }
    }
}

/// Sending end: every packet goes to the one address given to `new`.
pub struct UdpClient {
    socket: BlockingUdpSocket,
}

impl UdpClient {
    pub fn new(target: &str) -> Self {
        // let the OS pick any free local port
        let bound = BlockingUdpSocket::bind("0.0.0.0:0");
        let socket = match bound {
            Ok(s) => s,
            Err(e) => panic!("bind local udp socket: {e}"),
        };

        // fix the destination once (also resolves hostnames); fatal if it fails
        let connected = socket.connect(target);
        match connected {
            Ok(_) => {}
            Err(e) => panic!("connect udp {target}: {e}"),
        }

        UdpClient { socket: socket }
    }

    // the local IP the OS chose for reaching the target; empty if unknown
    pub fn local_ip(&self) -> String {
        let addr = self.socket.local_addr();
        match addr {
            Ok(a) => a.ip().to_string(),
            Err(_) => String::new(),
        }
    }

    // sends one packet; logs and returns false on failure
    pub fn send(&self, bytes: &[u8]) -> bool {
        let sent = self.socket.send(bytes);
        match sent {
            Ok(_) => true,
            Err(e) => {
                eprintln!("udp: send failed: {e}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // binds a server on a free port and returns it with its real address
    async fn start_server() -> (UdpSocketServer, SocketAddr) {
        let server = UdpSocketServer::new("127.0.0.1:0").await;
        let addr = server.socket.local_addr().unwrap();
        (server, addr)
    }

    // a plain blocking socket on a free port, standing in for the receiving end
    fn blocking_receiver() -> (BlockingUdpSocket, String) {
        let socket = BlockingUdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let addr = socket.local_addr().unwrap().to_string();
        (socket, addr)
    }

    // server tests

    #[tokio::test]
    async fn server_receives_exact_bytes_and_sender() {
        let (mut server, addr) = start_server().await;
        let client = AsyncUdpSocket::bind("127.0.0.1:0").await.unwrap();

        client.send_to(b"hello", addr).await.unwrap();

        let (bytes, peer) = server.receive().await.unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(peer, client.local_addr().unwrap());
    }

    #[tokio::test]
    async fn server_receives_each_packet_separately() {
        let (mut server, addr) = start_server().await;
        let client = AsyncUdpSocket::bind("127.0.0.1:0").await.unwrap();

        client.send_to(b"first", addr).await.unwrap();
        client.send_to(b"second", addr).await.unwrap();

        let first = server.receive().await.unwrap().0.to_vec();
        let second = server.receive().await.unwrap().0.to_vec();
        assert_eq!(first, b"first");
        assert_eq!(second, b"second");
    }

    #[tokio::test]
    async fn server_receives_empty_packet_as_empty_bytes() {
        let (mut server, addr) = start_server().await;
        let client = AsyncUdpSocket::bind("127.0.0.1:0").await.unwrap();

        client.send_to(b"", addr).await.unwrap();

        let (bytes, _) = server.receive().await.unwrap();
        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn server_cuts_packet_bigger_than_buffer() {
        let (mut server, addr) = start_server().await;
        let client = AsyncUdpSocket::bind("127.0.0.1:0").await.unwrap();

        client.send_to(&vec![7u8; 10_000], addr).await.unwrap();

        let (bytes, _) = server.receive().await.unwrap();
        assert_eq!(bytes.len(), 8192);
    }

    #[tokio::test]
    #[should_panic(expected = "bind udp")]
    async fn server_binding_an_address_already_in_use_panics() {
        let (_server, addr) = start_server().await;

        UdpSocketServer::new(&addr.to_string()).await;
    }

    #[tokio::test]
    #[should_panic(expected = "bind udp")]
    async fn server_binding_an_invalid_address_panics() {
        UdpSocketServer::new("not-an-address").await;
    }

    // client tests

    #[test]
    fn client_sent_bytes_arrive_unchanged() {
        let (receiver, addr) = blocking_receiver();
        let client = UdpClient::new(&addr);

        assert!(client.send(b"hello"));

        let mut buf = [0u8; 64];
        let (len, _) = receiver.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"hello");
    }

    #[test]
    fn client_each_send_arrives_as_its_own_packet() {
        let (receiver, addr) = blocking_receiver();
        let client = UdpClient::new(&addr);

        client.send(b"first");
        client.send(b"second");

        let mut buf = [0u8; 64];
        let (len, _) = receiver.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"first");
        let (len, _) = receiver.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"second");
    }

    #[test]
    fn client_local_ip_is_the_address_used_to_reach_the_target() {
        let (_receiver, addr) = blocking_receiver();
        let client = UdpClient::new(&addr);

        assert_eq!(client.local_ip(), "127.0.0.1");
    }

    #[test]
    fn client_resolves_hostname_target() {
        let (receiver, addr) = blocking_receiver();
        let port = addr.rsplit(':').next().unwrap();
        let client = UdpClient::new(&format!("localhost:{port}"));

        assert!(client.send(b"hi"));

        let mut buf = [0u8; 64];
        let (len, _) = receiver.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"hi");
    }

    #[test]
    fn client_packet_too_big_for_udp_returns_false() {
        let (_receiver, addr) = blocking_receiver();
        let client = UdpClient::new(&addr);

        assert!(!client.send(&vec![0u8; 70_000]));
    }

    #[test]
    fn client_sending_when_nobody_listens_does_not_panic() {
        let (receiver, addr) = blocking_receiver();
        drop(receiver);
        let client = UdpClient::new(&addr);

        // may be true or false: the OS can report "refused" on a later send
        client.send(b"anyone?");
        client.send(b"anyone?");
    }

    #[test]
    #[should_panic(expected = "connect udp")]
    fn client_invalid_target_panics() {
        UdpClient::new("not-an-address");
    }

    // both ends together, the way the reporter and daemon use them

    #[tokio::test]
    async fn client_packet_reaches_server() {
        let (mut server, addr) = start_server().await;
        let client = UdpClient::new(&addr.to_string());

        assert!(client.send(b"{\"device_id\":\"x\"}"));

        let (bytes, _) = server.receive().await.unwrap();
        assert_eq!(bytes, b"{\"device_id\":\"x\"}");
    }
}
