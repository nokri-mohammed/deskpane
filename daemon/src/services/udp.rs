//! Both ends of a UDP link: `UdpSocketServer` receives packets and answers
//! them (async, used by the daemon) and `UdpClient` sends to one address and
//! reads its replies without waiting (blocking, used by the reporter). What
//! the bytes mean is the caller's job.

use std::net::SocketAddr;
use std::net::UdpSocket as BlockingUdpSocket;

use tokio::net::UdpSocket as AsyncUdpSocket;

// largest payload the server reads (about one Ethernet frame); longer packets are cut
pub const MAX_PACKET_BYTES: usize = 1500;

/// Receiving end: bind once, then receive one packet at a time.
pub struct UdpSocketServer {
    socket: AsyncUdpSocket,
    buf: Vec<u8>,
}

impl UdpSocketServer {
    /*
     * binds the socket; returns the error so the caller decides what it means
     * (for example "port in use" gets its own message and exit code)
     *
     * addr: address to listen on, e.g. 0.0.0.0:8765
     */
    pub async fn try_new(addr: &str) -> Result<Self, std::io::Error> {
        let bound = AsyncUdpSocket::bind(addr).await;
        let socket = match bound {
            Ok(s) => s,
            Err(e) => return Err(e),
        };

        println!("udp: listening on {addr}");

        let server = UdpSocketServer {
            socket: socket,
            buf: vec![0u8; MAX_PACKET_BYTES],
        };
        Ok(server)
    }

    // the address the socket is bound to; None if the OS cannot say
    pub fn local_addr(&self) -> Option<SocketAddr> {
        let addr = self.socket.local_addr();
        match addr {
            Ok(a) => Some(a),
            Err(_) => None,
        }
    }

    // sends one packet to peer; logs and returns false on failure
    pub async fn send_to(&self, bytes: &[u8], peer: SocketAddr) -> bool {
        let sent = self.socket.send_to(bytes, peer).await;
        match sent {
            Ok(_) => true,
            Err(e) => {
                eprintln!("udp: send to {peer} failed: {e}");
                false
            }
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

/// What one non-blocking read on the client found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    // a packet of this many bytes is in the buffer
    Packet(usize),
    // nothing waiting right now
    Nothing,
    // the target answered "port unreachable": nobody listens there
    Refused,
}

/*
 * what a failed non-blocking read means: nothing waiting, or the target is
 * not listening (Linux says ConnectionRefused, Windows says ConnectionReset
 * for the same ICMP "port unreachable"); anything else counts as nothing
 */
fn classify_recv_error(kind: std::io::ErrorKind) -> Received {
    match kind {
        std::io::ErrorKind::ConnectionRefused => Received::Refused,
        std::io::ErrorKind::ConnectionReset => Received::Refused,
        _ => Received::Nothing,
    }
}

/// Sending end: every packet goes to the one address given to `new`, and
/// only packets from that address are received.
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

    /*
     * reads one waiting packet without blocking. The socket is connected, so
     * the OS only delivers packets from the target address.
     *
     * buf: where the packet goes (longer packets are cut to its size)
     * returns: Packet(len), Nothing, or Refused when the target is not listening
     */
    pub fn try_receive(&self, buf: &mut [u8]) -> Received {
        // non-blocking only for this one read, so send keeps its normal behaviour
        let switched = self.socket.set_nonblocking(true);
        match switched {
            Ok(_) => {}
            Err(e) => {
                eprintln!("udp: cannot switch to non-blocking: {e}");
                return Received::Nothing;
            }
        }

        let read = self.socket.recv(buf);

        let restored = self.socket.set_nonblocking(false);
        match restored {
            Ok(_) => {}
            Err(e) => eprintln!("udp: cannot switch back to blocking: {e}"),
        }

        match read {
            Ok(len) => Received::Packet(len),
            Err(e) => {
                let received = classify_recv_error(e.kind());
                if e.kind() != std::io::ErrorKind::WouldBlock && received == Received::Nothing {
                    eprintln!("udp: recv error: {e}");
                }
                received
            }
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
    use std::io::ErrorKind;
    use std::time::Duration;

    // binds a server on a free port and returns it with its real address
    async fn start_server() -> (UdpSocketServer, SocketAddr) {
        let server = UdpSocketServer::try_new("127.0.0.1:0").await.unwrap();
        let addr = server.socket.local_addr().unwrap();
        (server, addr)
    }

    // a plain blocking socket on a free port, standing in for the receiving end
    fn blocking_receiver() -> (BlockingUdpSocket, String) {
        let socket = BlockingUdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
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
    async fn server_receives_packet_of_exactly_buffer_size_whole() {
        let (mut server, addr) = start_server().await;
        let client = AsyncUdpSocket::bind("127.0.0.1:0").await.unwrap();

        client.send_to(&vec![7u8; 1500], addr).await.unwrap();

        let (bytes, _) = server.receive().await.unwrap();
        assert_eq!(bytes.len(), 1500);
    }

    #[tokio::test]
    async fn server_cuts_packet_bigger_than_buffer() {
        let (mut server, addr) = start_server().await;
        let client = AsyncUdpSocket::bind("127.0.0.1:0").await.unwrap();

        client.send_to(&vec![7u8; 2000], addr).await.unwrap();

        // the extra bytes are dropped, not reported as an error
        let (bytes, _) = server.receive().await.unwrap();
        assert_eq!(bytes.len(), 1500);
    }

    #[tokio::test]
    async fn try_new_on_free_port_succeeds() {
        let bound = UdpSocketServer::try_new("127.0.0.1:0").await;
        assert!(bound.is_ok());
    }

    #[tokio::test]
    async fn try_new_on_port_in_use_returns_addr_in_use() {
        let (_server, addr) = start_server().await;

        let bound = UdpSocketServer::try_new(&addr.to_string()).await;

        match bound {
            Ok(_) => panic!("expected the second bind to fail"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse),
        }
    }

    #[tokio::test]
    async fn try_new_on_invalid_address_returns_error() {
        let bound = UdpSocketServer::try_new("not-an-address").await;
        assert!(bound.is_err());
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
    async fn server_local_addr_is_the_bound_address() {
        let (server, addr) = start_server().await;
        assert_eq!(server.local_addr(), Some(addr));
    }

    #[tokio::test]
    async fn server_send_to_reaches_the_client() {
        let (mut server, addr) = start_server().await;
        let client = UdpClient::new(&addr.to_string());
        client.send(b"report");
        let (_, peer) = server.receive().await.unwrap();

        assert!(server.send_to(b"reply", peer).await);

        // give the loopback a moment, then read without blocking
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut buf = [0u8; 64];
        let received = client.try_receive(&mut buf);
        assert_eq!(received, Received::Packet(5));
        assert_eq!(&buf[..5], b"reply");
    }

    #[tokio::test]
    async fn server_send_to_a_too_big_packet_returns_false() {
        let (server, _addr) = start_server().await;
        let peer: SocketAddr = "127.0.0.1:9".parse().unwrap();
        assert!(!server.send_to(&vec![0u8; 70_000], peer).await);
    }

    #[test]
    fn recv_error_would_block_is_nothing() {
        assert_eq!(
            classify_recv_error(ErrorKind::WouldBlock),
            Received::Nothing
        );
    }

    #[test]
    fn recv_error_connection_refused_is_refused() {
        // Linux: ICMP port unreachable on a connected socket
        assert_eq!(
            classify_recv_error(ErrorKind::ConnectionRefused),
            Received::Refused
        );
    }

    #[test]
    fn recv_error_connection_reset_is_refused() {
        // Windows reports the same ICMP error as WSAECONNRESET
        assert_eq!(
            classify_recv_error(ErrorKind::ConnectionReset),
            Received::Refused
        );
    }

    #[test]
    fn recv_error_anything_else_is_nothing() {
        assert_eq!(classify_recv_error(ErrorKind::Other), Received::Nothing);
        assert_eq!(
            classify_recv_error(ErrorKind::Interrupted),
            Received::Nothing
        );
    }

    #[test]
    fn client_try_receive_returns_nothing_when_no_packet() {
        let (_receiver, addr) = blocking_receiver();
        let client = UdpClient::new(&addr);

        let mut buf = [0u8; 64];
        assert_eq!(client.try_receive(&mut buf), Received::Nothing);
    }

    #[test]
    fn client_try_receive_returns_the_reply() {
        let (receiver, addr) = blocking_receiver();
        let client = UdpClient::new(&addr);
        client.send(b"ping");
        let mut buf = [0u8; 64];
        let (_, client_addr) = receiver.recv_from(&mut buf).unwrap();
        receiver.send_to(b"pong", client_addr).unwrap();
        std::thread::sleep(Duration::from_millis(50));

        let received = client.try_receive(&mut buf);

        assert_eq!(received, Received::Packet(4));
        assert_eq!(&buf[..4], b"pong");
        // the reply was taken, so the next read finds nothing
        assert_eq!(client.try_receive(&mut buf), Received::Nothing);
    }

    #[test]
    fn client_try_receive_ignores_packets_from_other_addresses() {
        let (_receiver, addr) = blocking_receiver();
        let client = UdpClient::new(&addr);
        let client_addr = client.socket.local_addr().unwrap();
        let stranger = BlockingUdpSocket::bind("127.0.0.1:0").unwrap();
        let target = format!("127.0.0.1:{}", client_addr.port());
        stranger.send_to(b"forged", target).unwrap();
        std::thread::sleep(Duration::from_millis(50));

        let mut buf = [0u8; 64];
        assert_eq!(client.try_receive(&mut buf), Received::Nothing);
    }

    #[test]
    fn client_try_receive_reports_refused_when_nobody_listens() {
        let (receiver, addr) = blocking_receiver();
        drop(receiver);
        let client = UdpClient::new(&addr);
        client.send(b"anyone?");
        std::thread::sleep(Duration::from_millis(50));

        let mut buf = [0u8; 64];
        assert_eq!(client.try_receive(&mut buf), Received::Refused);
    }

    #[test]
    fn client_still_sends_after_try_receive() {
        // try_receive switches to non-blocking only for its own read
        let (receiver, addr) = blocking_receiver();
        let client = UdpClient::new(&addr);
        let mut buf = [0u8; 64];
        client.try_receive(&mut buf);

        assert!(client.send(b"after"));
        let (len, _) = receiver.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"after");
    }

    #[tokio::test]
    async fn client_packet_reaches_server() {
        let (mut server, addr) = start_server().await;
        let client = UdpClient::new(&addr.to_string());

        assert!(client.send(b"{\"device_id\":\"x\"}"));

        let (bytes, _) = server.receive().await.unwrap();
        assert_eq!(bytes, b"{\"device_id\":\"x\"}");
    }
}
