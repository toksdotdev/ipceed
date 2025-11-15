mod broadcast;
use std::ffi::OsString;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use interprocess::local_socket::GenericFilePath;
use interprocess::local_socket::ToFsName;
use interprocess::local_socket::tokio::RecvHalf;
use interprocess::local_socket::tokio::SendHalf;
use interprocess::local_socket::tokio::Stream as LocalSocketStream;
use interprocess::local_socket::traits::tokio::Stream;
use ipceed_types::message::BroadcastMessage;
use ipceed_types::message::MessageToClient;
use ipceed_types::message::MessageToServer;
use ipceed_types::primitives::RequestId;
use ipceed_types::primitives::Topic;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::BufWriter;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::IpcError;
use crate::IpcResult;
pub use crate::client::broadcast::IpcBroadcastReceiver;
use crate::codecs::IpcCodec;

/// A receiver for subscription messages that deserializes data to the specified type
pub struct IpcSubscriptionReceiver<C, T> {
    rx: mpsc::UnboundedReceiver<Arc<Vec<u8>>>,
    _phantom_codec: PhantomData<C>,
    _phantom_data: PhantomData<T>,
}

impl<C, T> IpcSubscriptionReceiver<C, T>
where
    C: IpcCodec + 'static,
    T: for<'de> serde::Deserialize<'de>,
{
    fn new(rx: mpsc::UnboundedReceiver<Arc<Vec<u8>>>) -> Self {
        Self {
            rx,
            _phantom_codec: PhantomData,
            _phantom_data: PhantomData,
        }
    }

    /// Receive the next message from the subscription
    pub async fn recv(&mut self) -> Option<T> {
        match self.rx.recv().await {
            Some(data) => match C::deserialize(&data) {
                Ok(deserialized) => Some(deserialized),
                Err(err) => {
                    tracing::error!(?err, "Failed to deserialize subscription message");
                    None
                }
            },
            None => None,
        }
    }
}

/// IPC client for connecting to servers and exchanging messages.
///
/// The client supports multiple communication patterns:
/// - Request-Response: Send requests and await typed responses
/// - Publish-Subscribe: Publish to topics and subscribe to receive messages
/// - Broadcast: Receive server-initiated broadcast messages
pub struct IpcClient<C> {
    request_timeout: Duration,
    subscription_timeout: Duration,
    write_socket: Mutex<BufWriter<SendHalf>>,
    pending_requests: Arc<DashMap<RequestId, oneshot::Sender<Arc<Vec<u8>>>>>,

    /// Subcription requests that haven't been acked yet by the server.
    awaiting_subcription_acks: Arc<DashMap<RequestId, oneshot::Sender<()>>>,

    /// List of topics the client is subscribed to, alongside the corresponding
    /// channel for publishing messages to that topic.
    subscriptions: Arc<DashMap<Topic, mpsc::UnboundedSender<Arc<Vec<u8>>>>>,
    _read_task: tokio::task::JoinHandle<()>,

    _phantom_codec: PhantomData<C>,
}

impl<C: IpcCodec + 'static> IpcClient<C> {
    /// Connect to an IPC server at the given path.
    ///
    /// Returns a tuple of the client and a broadcast receiver for server-initiated messages.
    ///
    /// # Arguments
    ///
    /// * `path` - File system path to the IPC socket
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails or the socket path is invalid.
    pub async fn connect<T>(path: impl AsRef<Path>) -> IpcResult<(Self, IpcBroadcastReceiver<C, T>)>
    where
        T: for<'de> serde::Deserialize<'de> + Send + 'static,
    {
        let name = OsString::from(path.as_ref())
            .to_os_string()
            .to_fs_name::<GenericFilePath>()
            .unwrap();

        let socket = LocalSocketStream::connect(name).await?;
        let (read_socket, write_socket) = socket.split();
        let read_socket = BufReader::new(read_socket);
        let write_socket = Mutex::new(BufWriter::new(write_socket));

        let pending_requests = Arc::new(DashMap::new());
        let unacked_subcriptions = Arc::new(DashMap::new());
        let (broadcast_tx, broadcast_rx) = unbounded_channel();
        let publish_tx = Arc::new(DashMap::new());
        let request_timeout = Duration::from_secs(30);
        let subscription_timeout = Duration::from_secs(30);

        let _read_task = Self::process_incoming_message(
            read_socket,
            pending_requests.clone(),
            unacked_subcriptions.clone(),
            broadcast_tx,
            publish_tx.clone(),
        );

        let client = Self {
            write_socket,
            pending_requests,
            awaiting_subcription_acks: unacked_subcriptions,
            subscriptions: publish_tx,
            _read_task,
            request_timeout,
            subscription_timeout,
            _phantom_codec: PhantomData,
        };

        Ok((client, IpcBroadcastReceiver::new(broadcast_rx)))
    }

    /// Set the timeout duration for requests.
    ///
    /// This timeout applies to [`send_request`](Self::send_request) calls when waiting for responses.
    /// Consumes and returns self to allow method chaining.
    ///
    /// # Arguments
    ///
    /// * `duration` - Maximum time to wait for a request response
    pub fn set_request_timeout(mut self, duration: Duration) -> Self {
        self.request_timeout = duration;
        self
    }

    /// Send a request and wait for a typed response with timeout.
    ///
    /// # Arguments
    ///
    /// * `data` - The request data to send
    pub async fn send_request<Request, Response>(&self, data: Request) -> IpcResult<Response>
    where
        Request: serde::Serialize,
        Response: for<'de> serde::Deserialize<'de>,
    {
        let data = C::serialize(&data)?;
        let message = MessageToServer::new_request(Arc::new(data));
        let id = message.request_id().expect("request to have an id");

        let (tx, rx) = oneshot::channel();
        self.pending_requests.insert(id.clone(), tx);
        self.send_message(&message).await?;

        match timeout(self.request_timeout, rx).await {
            Ok(Ok(response)) => {
                let deserialized = C::deserialize(&response)?;
                Ok(deserialized)
            }
            Ok(Err(_)) => {
                self.pending_requests.remove(&id);
                Err(IpcError::RequestFailed)
            }
            Err(_) => {
                self.pending_requests.remove(&id);
                Err(IpcError::Timeout)
            }
        }
    }

    /// Publish a message to a topic (fire-and-forget).
    ///
    /// The message will be delivered to all subscribers of the topic except the publisher.
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic to publish to
    /// * `data` - The message data to publish
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails or the connection is closed.
    pub async fn publish<P>(&self, topic: impl Into<Topic>, data: P) -> IpcResult<()>
    where
        P: serde::Serialize,
    {
        let data = C::serialize(&data)?;
        let msg = MessageToServer::new_publish(topic.into(), Arc::new(data));
        self.send_message(&msg).await
    }

    /// Subscribe to a topic and return a receiver for messages.
    ///
    /// This blocks until the server acknowledges the subscription or times out.
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic to subscribe to
    ///
    /// # Returns
    ///
    /// A receiver for messages published to this topic
    ///
    /// # Errors
    ///
    /// - [`IpcError::Timeout`] if subscription not acknowledged within timeout
    /// - [`IpcError::RequestFailed`] if the acknowledgment channel closed
    pub async fn subscribe<IncomingMessage>(
        &self,
        topic: impl Into<Topic>,
    ) -> IpcResult<IpcSubscriptionReceiver<C, IncomingMessage>>
    where
        IncomingMessage: for<'de> serde::Deserialize<'de>,
    {
        let topic = topic.into();
        let message = MessageToServer::new_subscribe(topic.clone());
        let id = message.request_id().expect("request to have an id");

        let (ack_tx, ack_rx) = oneshot::channel();
        self.awaiting_subcription_acks.insert(id, ack_tx);
        self.send_message(&message).await?;
        tracing::debug!(?topic, "submitted subscription request");

        match timeout(self.subscription_timeout, ack_rx).await {
            Ok(Ok(_)) => {
                tracing::debug!(?topic, "subscription successful");
                let (tx, rx) = mpsc::unbounded_channel();
                self.subscriptions.insert(topic, tx);
                Ok(IpcSubscriptionReceiver::new(rx))
            }
            Ok(Err(_)) => {
                self.awaiting_subcription_acks.remove(&id);
                Err(IpcError::RequestFailed)
            }
            Err(_) => {
                self.awaiting_subcription_acks.remove(&id);
                Err(IpcError::Timeout)
            }
        }
    }

    /// Unsubscribe from a topic.
    ///
    /// Stops receiving messages for the given topic and notifies the server.
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic to unsubscribe from
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be sent to the server.
    pub async fn unsubscribe(&self, topic: impl Into<Topic>) -> IpcResult<()> {
        let topic = topic.into();
        let message = MessageToServer::new_unsubscribe(topic.clone());
        self.send_message(&message).await?;
        self.subscriptions.remove(&topic);
        Ok(())
    }

    // async fn unsubscribe_from_all_topics(&self) {
    //     join_all(self.subscriptions.iter().map(|subscription| {
    //         let topic = subscription.key().clone();
    //         self.unsubscribe(topic)
    //     }))
    //     .await;
    // }

    /// Send a message to the IPC server.
    ///
    /// Serializes the message using the configured codec and sends it with a length prefix.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to send
    ///
    /// # Errors
    ///
    /// - [`IpcError::Serde`] if serialization fails
    /// - [`IpcError::Io`] if writing to the socket fails
    #[tracing::instrument(skip(self, message), level = "debug", fields(id = ?message.request_id()))]
    async fn send_message(&self, message: &MessageToServer) -> IpcResult<()> {
        let buffer = C::serialize(message)?;
        let length = buffer.len() as u32;

        let mut socket = self.write_socket.lock().await;
        socket.write_all(&length.to_be_bytes()).await?;
        socket.write_all(&buffer).await?;
        socket.flush().await?;
        drop(socket);

        tracing::trace!("Sent message");
        Ok(())
    }

    /// Read a message from the socket.
    ///
    /// Reads a length-prefixed message and deserializes it.
    async fn read_message(socket: &mut BufReader<RecvHalf>) -> IpcResult<MessageToClient> {
        let mut length_bytes = [0u8; 4];
        socket.read_exact(&mut length_bytes).await?;
        let length = u32::from_be_bytes(length_bytes);

        let mut buffer = vec![0u8; length as usize];
        socket.read_exact(&mut buffer).await?;
        C::deserialize(&buffer)
    }

    /// Spawn a task to process incoming messages from the server.
    ///
    /// Routes messages to appropriate handlers:
    /// - Responses to pending request handlers
    /// - Broadcasts to the broadcast channel
    /// - Published messages to topic subscription channels
    /// - Subscription acknowledgments to subscription waiters
    fn process_incoming_message(
        mut read_socket: BufReader<RecvHalf>,
        pending_requests: Arc<DashMap<RequestId, oneshot::Sender<Arc<Vec<u8>>>>>,
        unacked_subscriptions: Arc<DashMap<RequestId, oneshot::Sender<()>>>,
        broadcast_tx: mpsc::UnboundedSender<Arc<Vec<u8>>>,
        publish_tx: Arc<DashMap<Topic, mpsc::UnboundedSender<Arc<Vec<u8>>>>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let incoming_message = match Self::read_message(&mut read_socket).await {
                    Ok(msg) => msg,
                    Err(IpcError::Timeout) => break,
                    Err(err) => {
                        tracing::error!(?err, "Failed to read message");
                        continue;
                    }
                };

                tracing::trace!(id=?incoming_message.request_id(), "Received message");
                match incoming_message {
                    MessageToClient::Response {
                        id: request_id,
                        data,
                    } => {
                        let Some((_, tx)) = pending_requests.remove(&request_id) else {
                            tracing::warn!(?request_id, "Received response for unknown request");
                            continue;
                        };
                        if let Err(_) = tx.send(data) {
                            tracing::warn!(
                                ?request_id,
                                "Failed to send response to waiting request"
                            );
                        }
                    }
                    MessageToClient::Broadcast(BroadcastMessage {
                        id: request_id,
                        data,
                    }) => {
                        if let Err(_) = broadcast_tx.send(data) {
                            tracing::warn!(?request_id, "Failed to send broadcast message");
                        }
                    }
                    MessageToClient::PublishedMessage { topic, data } => {
                        if let Some(tx) = publish_tx.get(&topic) {
                            if let Err(_) = tx.send(data) {
                                tracing::warn!(?topic, "Failed to publish message to topic");
                            }
                        }
                    }
                    MessageToClient::SubscriptionAcked { id: request_id } => {
                        let Some((_, tx)) = unacked_subscriptions.remove(&request_id) else {
                            tracing::warn!(?request_id, "Received unknown ack");
                            continue;
                        };
                        if let Err(_) = tx.send(()) {
                            tracing::warn!(?request_id, "Failed to propagate subscription ack");
                        }
                    }
                }
            }
        })
    }
}

// impl Drop for IpcClient {
//     fn drop(&mut self) {
//         Handle::current().block_on(async {
//             self.unsubscribe_from_all_topics().await;
//         });
//         // });
//     }
// }
