use serde::Serialize;
use tokio::sync::broadcast;

/// In-process pub/sub: anyone can subscribe, and `send_json` delivers one
/// value as a JSON string to every current subscriber.
pub struct Broadcaster {
    tx: broadcast::Sender<String>,
}

impl Broadcaster {
    // capacity = how many unread messages a slow subscriber can fall behind
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel::<String>(capacity);
        Broadcaster { tx: tx }
    }

    // gives a new listener that receives every message sent from now on
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    // serializes the value to JSON and sends it; dropped if that fails or nobody listens
    pub fn send_json<T: Serialize>(&self, value: &T) {
        let serialized = serde_json::to_string(value);
        let line = match serialized {
            Ok(l) => l,
            Err(_) => return,
        };

        let sent = self.tx.send(line);
        match sent {
            Ok(_) => {}
            Err(_) => {} // no subscribers yet is fine
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::sync::broadcast::error::RecvError;

    #[tokio::test]
    async fn subscriber_receives_value_as_json() {
        let broadcaster = Broadcaster::new(16);
        let mut rx = broadcaster.subscribe();

        broadcaster.send_json(&vec![1, 2, 3]);

        assert_eq!(rx.recv().await.unwrap(), "[1,2,3]");
    }

    #[tokio::test]
    async fn every_subscriber_gets_the_same_message() {
        let broadcaster = Broadcaster::new(16);
        let mut rx_a = broadcaster.subscribe();
        let mut rx_b = broadcaster.subscribe();

        broadcaster.send_json(&"hello");

        assert_eq!(rx_a.recv().await.unwrap(), "\"hello\"");
        assert_eq!(rx_b.recv().await.unwrap(), "\"hello\"");
    }

    #[tokio::test]
    async fn sending_with_no_subscribers_does_not_panic() {
        let broadcaster = Broadcaster::new(16);

        broadcaster.send_json(&"nobody is listening");
    }

    #[tokio::test]
    async fn late_subscriber_only_gets_messages_sent_after_subscribing() {
        let broadcaster = Broadcaster::new(16);
        broadcaster.send_json(&"before");

        let mut rx = broadcaster.subscribe();
        broadcaster.send_json(&"after");

        assert_eq!(rx.recv().await.unwrap(), "\"after\"");
    }

    #[tokio::test]
    async fn value_that_cannot_become_json_is_dropped() {
        let broadcaster = Broadcaster::new(16);
        let mut rx = broadcaster.subscribe();

        // JSON object keys must be strings, so a tuple key fails to serialize
        let mut bad: HashMap<(i32, i32), i32> = HashMap::new();
        bad.insert((1, 2), 3);
        broadcaster.send_json(&bad);
        broadcaster.send_json(&"ok");

        assert_eq!(rx.recv().await.unwrap(), "\"ok\"");
    }

    #[tokio::test]
    async fn slow_subscriber_misses_oldest_messages_when_over_capacity() {
        let broadcaster = Broadcaster::new(1);
        let mut rx = broadcaster.subscribe();

        broadcaster.send_json(&1);
        broadcaster.send_json(&2);

        assert_eq!(rx.recv().await, Err(RecvError::Lagged(1)));
        assert_eq!(rx.recv().await.unwrap(), "2");
    }
}
