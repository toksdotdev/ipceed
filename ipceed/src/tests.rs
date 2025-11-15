use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures::future::pending;
use ipceed_types::primitives::ClientId;
use ipceed_types::primitives::RequestId;
use ipceed_types::primitives::Topic;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio_util::task::JoinMap;

use crate::IpcResult;
use crate::client::IpcBroadcastReceiver;
use crate::client::IpcClient;
use crate::codecs::IpcCodec;
use crate::codecs::JsonCodec;
use crate::server::IpcBroadcastSender;
use crate::server::IpcServer;
use crate::server::handler::MessageHandler;

/// Mock receiver for listening for unsubscribed events.
type UnsubscribedReceiver = Receiver<Topic>;

struct MockServerHandler {
    requests: Arc<Mutex<Vec<(String, MockRequest)>>>,
    publishes: Arc<Mutex<HashSet<Topic>>>,
    unsubscribed_tx: Sender<Topic>,
}

impl MockServerHandler {
    fn new(unsubscribed_tx: Sender<Topic>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            publishes: Arc::new(Mutex::new(HashSet::new())),
            unsubscribed_tx,
        }
    }

    async fn get_request_log(&self) -> Vec<(String, MockRequest)> {
        self.requests.lock().await.clone()
    }

    async fn get_publishes(&self) -> HashSet<Topic> {
        self.publishes.lock().await.clone()
    }
}

#[derive(Serialize, Deserialize)]
struct MockResponse {
    data: String,
}

#[async_trait::async_trait]
impl<C: IpcCodec> MessageHandler<C, MockRequest, MockResponse> for MockServerHandler {
    async fn handle_request(&self, id: RequestId, data: MockRequest) -> Option<MockResponse> {
        self.requests
            .lock()
            .await
            .push((id.to_string(), data.clone()));

        Some(MockResponse {
            // Echo back the data with "response:" prefix
            data: format!("response:{}", data.data),
        })
    }

    async fn on_publish(&self, topic: &Topic, _data: Arc<Vec<u8>>) {
        self.publishes.lock().await.insert(topic.clone());
    }

    async fn on_unsubscribe(&self, _: ClientId, topic: &Topic) {
        self.unsubscribed_tx
            .send(topic.to_owned())
            .await
            .expect("Failed to send unsubscribe notification");
    }
}

async fn mock_server(
    addr: &str,
) -> (
    Arc<MockServerHandler>,
    UnsubscribedReceiver,
    IpcBroadcastSender<JsonCodec>,
) {
    let (unsubscribed_tx, unsubscribed_rx) = tokio::sync::mpsc::channel(10);
    let handler = MockServerHandler::new(unsubscribed_tx);
    let handler = Arc::new(handler);
    let mut server = IpcServer::new(addr.to_string(), handler.clone(), 100).unwrap();
    let broadcaster = server.broadcaster();
    tokio::spawn(async move {
        loop {
            server.poll().await.unwrap();
        }
    });
    (handler, unsubscribed_rx, broadcaster)
}

async fn mock_client(
    addr: &str,
) -> (
    IpcClient<JsonCodec>,
    IpcBroadcastReceiver<JsonCodec, Vec<u8>>,
) {
    let (client, broadcast_rx) = IpcClient::connect::<Vec<u8>>(addr)
        .await
        .expect("Failed to connect");
    (
        client.set_request_timeout(Duration::from_secs(5)),
        broadcast_rx,
    )
}

/// Create a non-existent socker path we can start listening on.
fn mock_socket_address() -> String {
    let file = tempfile::Builder::new().tempfile().unwrap();
    let path = file.path().to_str().unwrap().to_string();
    drop(file);
    path
}

async fn mock_client_server() -> (
    Arc<MockServerHandler>,
    IpcClient<JsonCodec>,
    UnsubscribedReceiver,
    IpcBroadcastSender<JsonCodec>,
    IpcBroadcastReceiver<JsonCodec, Vec<u8>>,
) {
    let sock_addr = mock_socket_address();
    let (server, unsubscribe_rx, broadcast_tx) = mock_server(&sock_addr).await;
    let (client, broadcast_rx) = mock_client(&sock_addr).await;
    (server, client, unsubscribe_rx, broadcast_tx, broadcast_rx)
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
struct MockRequest {
    data: String,
}

#[tokio::test]
async fn test_request_response() {
    let (server, client, ..) = mock_client_server().await;

    let data = MockRequest {
        data: "test request data".to_string(),
    };
    let response = client
        .set_request_timeout(Duration::from_secs(5))
        .send_request::<MockRequest, MockResponse>(data.clone())
        .await
        .expect("Failed to send request");

    assert_eq!(response.data, format!("response:{}", data.data));
    let requests = server.get_request_log().await;
    assert_eq!(requests.len(), 1); // 1 request issued in total
    assert_eq!(requests[0].1, data);
}

#[tokio::test]
async fn test_multiple_requests() {
    let (server, client, ..) = mock_client_server().await;
    let client = Arc::new(client);
    let mut tasks = JoinMap::new();

    for i in 0..10 {
        let client = client.clone();
        tasks.spawn(i.to_string(), async move {
            let result: IpcResult<MockResponse> = client
                .send_request(MockRequest {
                    data: i.to_string(),
                })
                .await;
            result
        });
    }

    while let Some((i, response)) = tasks.join_next().await {
        assert_eq!(response.unwrap().unwrap().data, format!("response:{i}"));
    }

    let requests = server.get_request_log().await;
    assert_eq!(requests.len(), 10);
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct MockTopicMessage {
    message: String,
}

#[tokio::test]
async fn test_publish_subscribe() {
    let addr = mock_socket_address();
    let (server, ..) = mock_server(&addr).await;
    let (client1, _) = mock_client(&addr).await;
    let (client2, _) = mock_client(&addr).await;
    let (client3, _) = mock_client(&addr).await;

    // Subscribe to different topics
    let mut client1_topic1_rx = client1
        .subscribe::<MockTopicMessage>("topic1".to_string())
        .await
        .expect("Failed to subscribe");
    let mut client3_topic1_rx = client3
        .subscribe::<MockTopicMessage>("topic1".to_string())
        .await
        .expect("Failed to subscribe");
    let mut client2_topic2_rx = client2
        .subscribe::<MockTopicMessage>("topic2".to_string())
        .await
        .expect("Failed to subscribe");

    // Publish to topic1
    let topic1_data = MockTopicMessage {
        message: "message for topic1".to_string(),
    };

    client1
        .publish("topic1".to_string(), topic1_data.clone())
        .await
        .expect("Failed to publish");

    // Publish to topic2
    let topic2_data = MockTopicMessage {
        message: "message for topic2".to_string(),
    };
    client2
        .publish("topic2".to_string(), topic2_data.clone())
        .await
        .expect("Failed to publish");

    // client1 shouldn't receive any topic1 message because it published the only
    // message to that topic.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), client1_topic1_rx.recv())
            .await
            .is_err()
    );

    // Client2 shouldn't receive any topic2 message because it published the only
    // message to that topic.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), client2_topic2_rx.recv())
            .await
            .is_err()
    );

    // Client3 should receive topic1 message sent from client1, afterwards, nothing else!!!
    let received = tokio::time::timeout(Duration::from_millis(200), client3_topic1_rx.recv())
        .await
        .expect("Timeout waiting for topic1 message")
        .expect("Channel closed");
    assert_eq!(received, topic1_data);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), client3_topic1_rx.recv())
            .await
            .is_err()
    );

    let publishes = server.get_publishes().await;
    assert_eq!(publishes.len(), 2);
    assert!(publishes.contains(&Topic::from("topic1")));
    assert!(publishes.contains(&Topic::from("topic2")));
}

#[tokio::test]
async fn test_unsubscribe() {
    let addr = mock_socket_address();
    let (_server, mut unsubscribed_rx, _) = mock_server(&addr).await;
    let (client1, _) = mock_client(&addr).await;
    let (client2, _) = mock_client(&addr).await;

    // Subscribe to topic
    let topic = Topic::from("test_topic");
    let mut topic_rx = client1
        .subscribe::<Vec<u8>>(topic.clone())
        .await
        .expect("Failed to subscribe");

    // Publish first message
    let data = b"first message".to_vec();
    client2
        .publish(topic.clone(), data.clone())
        .await
        .expect("Failed to publish");

    // Receive first message
    let received = tokio::time::timeout(Duration::from_millis(200), topic_rx.recv())
        .await
        .expect("Timeout waiting for first message")
        .expect("Channel closed");
    assert_eq!(received, data);

    // Unsubscribe
    client1
        .unsubscribe(topic.clone())
        .await
        .expect("Failed to unsubscribe");

    let unsubscribed = unsubscribed_rx.recv().await;
    assert_eq!(unsubscribed.unwrap(), topic);

    // Publish second message
    let data2 = b"second message".to_vec();
    client2
        .publish(topic.clone(), data2.clone())
        .await
        .expect("Failed to publish");

    // Should not receive second message
    assert!(
        topic_rx.recv().await.is_none(),
        "Should not receive message after unsubscribe"
    );
}

#[tokio::test(start_paused = true)]
async fn test_request_timeout() {
    // Create a server that stuck on pending forever for requests.
    struct PendForever;
    #[async_trait::async_trait]
    impl<C: IpcCodec> MessageHandler<C, String, String> for PendForever {
        async fn handle_request(&self, _id: RequestId, _data: String) -> Option<String> {
            pending().await
        }
    }

    let addr = mock_socket_address();
    let server = Arc::new(PendForever);
    let mut server = IpcServer::<JsonCodec, String, String>::new(&addr, server, 100).unwrap();
    tokio::spawn(async move {
        loop {
            server.poll().await.unwrap();
        }
    });

    let (client, _) = mock_client(&addr).await;

    let client = client.set_request_timeout(Duration::from_secs(10));
    let result: Result<Vec<u8>, _> = client.send_request(b"test".to_vec()).await;
    assert!(matches!(result.unwrap_err(), crate::IpcError::Timeout));
}

#[tokio::test]
async fn test_broadcast() {
    let addr = mock_socket_address();
    let (_server, _, broadcast_tx) = mock_server(&addr).await;
    let (_client1, mut broadcast1_rx) = mock_client(&addr).await;
    let (_client2, mut broadcast2_rx) = mock_client(&addr).await;

    broadcast_tx
        .broadcast(b"test broadcast".to_vec())
        .expect("broadcast to succeed");

    // Client1 should receive the broadcast message
    let received = tokio::time::timeout(Duration::from_millis(200), broadcast1_rx.recv())
        .await
        .expect("Timeout waiting for broadcast")
        .expect("Channel closed");
    assert_eq!(received, b"test broadcast".to_vec());
    // Client2 should also receive the broadcast message
    let received = tokio::time::timeout(Duration::from_millis(200), broadcast2_rx.recv())
        .await
        .expect("Timeout waiting for broadcast")
        .expect("Channel closed");
    assert_eq!(received, b"test broadcast".to_vec());

    // No more messages should be received
    assert!(
        tokio::time::timeout(Duration::from_millis(200), broadcast1_rx.recv())
            .await
            .is_err(),
        "No more messages should be received after broadcast"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), broadcast2_rx.recv())
            .await
            .is_err(),
        "No more messages should be received after broadcast"
    );
}
