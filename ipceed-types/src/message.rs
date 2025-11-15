use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;

use crate::primitives::RequestId;
use crate::primitives::Topic;

/// Messages sent from client to server.
///
/// Represents all possible message types in the client-to-server direction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageToServer {
    /// Send a request to the server, which is expected to respond
    /// with a [`MessageToClient::Response`].
    ///
    /// The `data` is the serialized request data.
    Request { id: RequestId, data: Arc<Vec<u8>> },
    /// Publish a message to a topic.
    /// This is sent to all subscribers of the topic as a [`MessageToClient::PublishedMessage`]
    /// except for the client that published it.
    ///
    /// The `data` is the serialized message content.
    Publish { topic: Topic, data: Arc<Vec<u8>> },
    /// Subscribe to a topic.
    /// The server will respond with a [`MessageToClient::SubscriptionAcked`] message
    /// if the subscription is successful.
    Subscribe { id: RequestId, topic: Topic },
    /// Unsubscribe from a topic.
    Unsubscribe { topic: Topic },
}

/// Messages sent from server to client.
///
/// Represents all possible message types in the server-to-client direction.
#[derive(Debug, Serialize, Deserialize)]
pub enum MessageToClient {
    /// Request message sent by the client
    Response { id: RequestId, data: Arc<Vec<u8>> },
    /// Broadcast message sent by the server.
    Broadcast(BroadcastMessage),
    /// Message published to a topic
    /// This is sent to all subscribers of the topic
    PublishedMessage { topic: Topic, data: Arc<Vec<u8>> },
    /// Acknowledgment for a subscription request
    SubscriptionAcked { id: RequestId },
}

/// A broadcast message sent from server to all connected clients.
///
/// Broadcast messages are server-initiated and delivered to all active connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastMessage {
    pub id: RequestId,
    pub data: Arc<Vec<u8>>,
}

impl BroadcastMessage {
    /// Create a new broadcast message with generated ID.
    ///
    /// # Arguments
    ///
    /// * `data` - The serialized message data
    pub fn new(data: Arc<Vec<u8>>) -> Self {
        Self {
            id: RequestId::new(),
            data,
        }
    }

    /// Consume the message and return the data.
    pub fn into_data(self) -> Arc<Vec<u8>> {
        self.data
    }
}

impl MessageToServer {
    /// Create a new request message with generated ID
    pub fn new_request(data: Arc<Vec<u8>>) -> Self {
        Self::Request {
            id: RequestId::new(),
            data,
        }
    }

    /// Create a publish message
    pub fn new_publish(topic: Topic, data: Arc<Vec<u8>>) -> Self {
        Self::Publish { topic, data }
    }

    /// Create a subscribe message
    pub fn new_subscribe(topic: Topic) -> Self {
        Self::Subscribe {
            id: RequestId::new(),
            topic,
        }
    }

    /// Create an unsubscribe message
    pub fn new_unsubscribe(topic: Topic) -> Self {
        Self::Unsubscribe { topic }
    }

    /// Get the request ID if this message has one.
    ///
    /// Returns `Some` for Request and Subscribe messages, `None` otherwise.
    pub fn request_id(&self) -> Option<RequestId> {
        match self {
            MessageToServer::Request { id, .. } | MessageToServer::Subscribe { id, .. } => {
                Some(id.clone())
            }
            _ => None,
        }
    }
}

impl From<BroadcastMessage> for MessageToClient {
    fn from(msg: BroadcastMessage) -> Self {
        MessageToClient::Broadcast(msg)
    }
}

impl MessageToClient {
    /// Create a new response message
    pub fn new_response(id: RequestId, data: Arc<Vec<u8>>) -> Self {
        Self::Response { id, data }
    }

    /// Create a new published message
    pub fn new_published(topic: Topic, data: Arc<Vec<u8>>) -> Self {
        Self::PublishedMessage { topic, data }
    }

    /// Get the request ID if this message has one.
    ///
    /// Returns `Some` for Response messages, `None` otherwise.
    pub fn request_id(&self) -> Option<RequestId> {
        match self {
            MessageToClient::Response { id, .. } => Some(id.clone()),
            _ => None,
        }
    }
}
