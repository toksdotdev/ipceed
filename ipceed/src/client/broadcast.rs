use std::marker::PhantomData;
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::codecs::IpcCodec;

/// Receiver for server-initiated broadcast messages.
///
/// This receiver deserializes messages from the server that are sent to all connected clients.
pub struct IpcBroadcastReceiver<C, T> {
    rx: UnboundedReceiver<Arc<Vec<u8>>>,
    codec: PhantomData<C>,
    _phantom_data: PhantomData<T>,
}

impl<C, T> IpcBroadcastReceiver<C, T>
where
    C: IpcCodec,
    T: for<'de> serde::Deserialize<'de>,
{
    pub fn new(rx: UnboundedReceiver<Arc<Vec<u8>>>) -> Self {
        Self {
            rx,
            codec: PhantomData,
            _phantom_data: PhantomData,
        }
    }

    /// Receive the next broadcast message
    pub async fn recv(&mut self) -> Option<T> {
        match self.rx.recv().await {
            Some(data) => match C::deserialize(&data) {
                Ok(deserialized) => Some(deserialized),
                Err(err) => {
                    tracing::error!(?err, "Failed to deserialize broadcast message");
                    None
                }
            },
            None => None,
        }
    }
}
