use std::marker::PhantomData;
use std::sync::Arc;

use ipceed_types::message::BroadcastMessage;
use serde::Serialize;
use tokio::sync::broadcast::Sender;

use crate::IpcResult;
use crate::codecs::IpcCodec;

/// Broadcast messages to all connected clients.
#[derive(Debug, Clone)]
pub struct IpcBroadcastSender<C> {
    broadcast_tx: Sender<BroadcastMessage>,
    _phantom_codec: PhantomData<C>,
}

impl<C: IpcCodec> IpcBroadcastSender<C> {
    /// Create a new IPC handle for sending broadcast messages
    pub fn new(broadcast_tx: Sender<BroadcastMessage>) -> Self {
        Self {
            broadcast_tx,
            _phantom_codec: PhantomData,
        }
    }

    /// Send a broadcast message to all connected clients
    ///
    /// # Returns
    /// The number of clients that received the message, or an error if the broadcast failed.
    pub fn broadcast<T: Serialize>(&self, message: T) -> IpcResult<usize> {
        let serialized = Arc::new(C::serialize(&message)?);
        let message = BroadcastMessage::new(serialized);

        match self.broadcast_tx.send(message) {
            Ok(connection_count) => {
                // The connection count maps directly to the number of active client connections.
                tracing::debug!(?connection_count, "Broadcast message sent to clients");
                Ok(connection_count)
            }
            Err(err) => {
                tracing::error!(?err, "Failed to send broadcast message");
                Err(crate::IpcError::BroadcastFailed)
            }
        }
    }
}
