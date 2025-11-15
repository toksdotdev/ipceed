use std::sync::Arc;

use async_trait::async_trait;
use ipceed_types::primitives::ClientId;
use ipceed_types::primitives::RequestId;
use ipceed_types::primitives::Topic;

use crate::codecs::IpcCodec;

/// Trait for handling incoming messages from clients.
#[async_trait]
pub trait MessageHandler<C, Request, Response>
where
    Self: Send + Sync,
    C: IpcCodec,
{
    /// Handle incoming request messages and optionally return a response
    async fn handle_request(&self, id: RequestId, request: Request) -> Option<Response>;

    /// Event triggered when a message is published to a topic
    /// This is called after the server has processed the publish request.
    ///
    /// This is called in-line, and could therefore block the processing of
    /// incoming messages for a specific client connection.
    async fn on_publish(&self, _topic: &Topic, _data: Arc<Vec<u8>>) {}

    /// Event triggered when a client unsubscribes from a topic
    /// This is called after the client has unsubscribed and the server has acknowledged it.
    async fn on_unsubscribe(&self, _client_id: ClientId, _topic: &Topic) {}
}
