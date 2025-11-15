# ipceed

Cross-platform async IPC crate with **multiple messaging patterns**.

## OS

- Windows
- Linux
- macOS

## Features

- **Flexible messaging**: request-response, broadcast, publish-subscribe, and server-initiated broadcast
- **Cross-platform**: Built on top of the [`interprocess`](https://docs.rs/interprocess/latest/interprocess/) crate.
- **Custom codecs**: Easily swap out message ser/de formats.

## Usage

### Server

The server is a long-running process that handles incoming connections and acts as a relay for messages between clients.

```rust
use std::sync::Arc;
use ipceed::{server::IpcServer, MessageHandler, RequestId, Topic, ClientId};
use ipceed::codecs::JsonCodec;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
struct MyRequest {
    message: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct MyResponse {
    reply: String,
}

struct MyHandler;

#[async_trait::async_trait]
impl<C: IpcCodec> MessageHandler<C, MyRequest, MyResponse> for MyHandler {
    async fn handle_request(&self, id: RequestId, request: &MyRequest) -> Option<MyResponse> {
        println!("Handling request {}: {:?}", id, request);
        Some(MyResponse {
            reply: format!("Echo: {}", request.message),
        })
    }

    async fn on_publish(&self, topic: &Topic, _data: Arc<Vec<u8>>) {
        println!("Message published to topic: {}", topic);
    }

    async fn on_unsubscribe(&self, client_id: ClientId, topic: &Topic) {
        println!("Client {} unsubscribed from topic: {}", client_id, topic);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let handler = Arc::new(MyHandler);
    let server = IpcServer::<JsonCodec, MyRequest, MyResponse>::new(
        "/tmp/my_ipc_server",
        handler,
        100, // broadcast capacity
    )?;

    // Get broadcaster for sending messages to all clients
    let broadcaster = server.broadcaster();

    // Start the server (this blocks)
    server.start().await;
    Ok(())
}
```

### Client: Request-Response

The client can send requests to the server and await responses.

```rust
use ipceed::{client::IpcClient, codecs::JsonCodec};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize, Debug)]
struct MyRequest {
    message: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct MyResponse {
    reply: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the server
    let (client, _broadcast_rx) = IpcClient::<JsonCodec>::connect::<Vec<u8>>("/tmp/my_ipc_server").await?;

    // Configure request timeout
    let client = client.set_request_timeout(Duration::from_secs(5));

    // Request-Response
    let request = MyRequest {
        message: "Hello, server!".to_string(),
    };
    let response: MyResponse = client.send_request(request).await?;
    println!("Response: {:?}", response);

    Ok(())
}
```

### Client: Publish-Subscribe

The client can subscribe to topics and receive messages published to the topic by other clients or the server.

```rust
use ipceed::{client::IpcClient, codecs::JsonCodec};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct TopicMessage {
    content: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the server
    let (client, mut broadcast_rx) = IpcClient::<JsonCodec>::connect::<Vec<u8>>("/tmp/my_ipc_server").await?;

    // Subscribe to a topic
    let mut subscription = client.subscribe::<TopicMessage>("my_topic").await?;

    // Publish to the topic
    let topic_message = TopicMessage {
        content: "Hello, subscribers!".to_string(),
    };
    client.publish("my_topic", topic_message).await?;

    // Receive published message
    if let Some(received) = subscription.recv().await {
        println!("Received topic message: {:?}", received);
    }

    // Unsubscribe from topic
    client.unsubscribe("my_topic").await?;

    // Listen for broadcast messages
    if let Some(broadcast) = broadcast_rx.recv().await {
        println!("Received broadcast: {:?}", broadcast);
    }

    Ok(())
}
```

### Server: Broadcasts

The server can send messages to all connected clients.

```rust
use ipceed::server::IpcBroadcastSender;
use ipceed::codecs::JsonCodec;

async fn broadcast_example(broadcaster: IpcBroadcastSender<JsonCodec>) {
    // Send a broadcast message to all connected clients
    let message = "Server announcement: System maintenance in 5 minutes";
    match broadcaster.broadcast(message) {
        Ok(client_count) => println!("Broadcast sent to {} clients", client_count),
        Err(e) => eprintln!("Failed to broadcast: {:?}", e),
    }
}
```

## Codec

Out of the box, ipceed provides the following codecs:

- **`JsonCodec`**: JSON via `serde_json`

You could also rely on other formats by implementing the `IpcCodec` trait:

```rust
use ipceed::codecs::IpcCodec;
use ipceed::IpcResult;

#[derive(Debug, Clone)]
pub struct CustomCodec;

impl IpcCodec for CustomCodec {
    fn serialize<T>(value: &T) -> IpcResult<Vec<u8>>
    where
        T: serde::Serialize,
    {
        // Custom serialization logic
        Ok(serde_json::to_vec(value)?)
    }

    fn deserialize<T>(bytes: &[u8]) -> IpcResult<T>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        // Custom deserialization logic
        Ok(serde_json::from_slice::<T>(bytes)?)
    }
}
```

## Future Enhancements

- Support for additional codecs (MessagePack, Protocol Buffers, etc.)
