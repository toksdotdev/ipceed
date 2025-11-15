use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Arc;

use interprocess::local_socket::tokio::RecvHalf;
use interprocess::local_socket::tokio::SendHalf;
use interprocess::local_socket::tokio::Stream as LocalSocketStream;
use interprocess::local_socket::traits::tokio::Stream;
use ipceed_types::message::BroadcastMessage;
use ipceed_types::message::MessageToClient;
use ipceed_types::message::MessageToServer;
use ipceed_types::primitives::ClientId;
use ipceed_types::primitives::Topic;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::BufWriter;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::broadcast::Receiver as BroadcastReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::IpcResult;
use crate::codecs::IpcCodec;
use crate::server::handler::MessageHandler;

/// Manages a single client connection on the server side.
///
/// Handles bidirectional communication with a client, including:
/// - Receiving and processing client messages
/// - Sending responses and published messages
/// - Managing topic subscriptions
/// - Forwarding broadcast messages
pub(crate) struct ClientConnection<C, Request, Response> {
    id: ClientId,
    read_socket: Mutex<BufReader<RecvHalf>>,
    write_socket: Mutex<BufWriter<SendHalf>>,
    publish_tx: UnboundedSender<(ClientId, Topic, Arc<Vec<u8>>)>,
    subscriptions: Arc<RwLock<HashSet<Topic>>>,
    _codec_phantom: PhantomData<C>,
    _request_phantom: PhantomData<Request>,
    _response_phantom: PhantomData<Response>,
}

impl<C, Req, Res> ClientConnection<C, Req, Res>
where
    C: IpcCodec,
    Req: for<'de> serde::Deserialize<'de> + Send + Sync + 'static,
    Res: serde::Serialize + Send + Sync + 'static,
{
    /// Create a new client connection.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for this client
    /// * `socket` - The socket stream for communicating with the client
    /// * `publish_tx` - Channel for forwarding published messages to the server
    pub fn new(
        id: ClientId,
        socket: LocalSocketStream,
        publish_tx: UnboundedSender<(ClientId, Topic, Arc<Vec<u8>>)>,
    ) -> Self {
        let (read_socket, write_socket) = socket.split();
        let read_socket = Mutex::new(BufReader::new(read_socket));
        let write_socket = Mutex::new(BufWriter::new(write_socket));

        Self {
            id,
            publish_tx,
            read_socket,
            write_socket,
            subscriptions: Arc::new(RwLock::new(HashSet::new())),
            _codec_phantom: PhantomData,
            _request_phantom: PhantomData,
            _response_phantom: PhantomData,
        }
    }

    /// Get the [`ClientId`] for the current connection.
    pub fn id(&self) -> ClientId {
        self.id
    }

    /// Check if the client is subscribed to a topic.
    pub async fn is_subscribed_to(&self, topic: &Topic) -> bool {
        self.subscriptions.read().await.contains(topic)
    }

    /// Subscribe the client to a topic.
    ///
    /// Adds the topic to the client's subscription set.
    async fn subscribe_to(&self, topic: &Topic) {
        self.subscriptions.write().await.insert(topic.clone());
    }

    /// Unsubscribe the client from a topic.
    ///
    /// Removes the topic from the client's subscription set.
    async fn unsubscribe_from(&self, topic: &Topic) {
        self.subscriptions.write().await.remove(topic);
    }

    /// Read a message from the client.
    ///
    /// Reads a length-prefixed message and deserializes it.
    ///
    /// # Errors
    ///
    /// Returns an error if reading fails or deserialization fails
    async fn read_message(&self) -> IpcResult<MessageToServer> {
        // read length
        let mut length_bytes = [0u8; 4];
        let mut socket = self.read_socket.lock().await;
        socket.read_exact(&mut length_bytes).await?;
        let length = u32::from_be_bytes(length_bytes);

        // read content with exact buffer length
        let mut buffer = vec![0u8; length as usize];
        socket.read_exact(&mut buffer).await?;
        drop(socket);

        C::deserialize(&buffer)
    }

    /// Send a message to the client.
    ///
    /// Serializes the message and sends it with a length prefix.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to send to the client
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or writing fails
    pub(crate) async fn send_message(&self, message: &MessageToClient) -> IpcResult<()> {
        let buffer = C::serialize(&message)?;
        let length = buffer.len() as u32;
        let client_id = self.id;

        let mut socket = self.write_socket.lock().await;
        socket.write_all(&length.to_be_bytes()).await?;
        socket.write_all(&buffer).await?;
        socket.flush().await?;
        drop(socket);

        tracing::debug!(?client_id, "Sent message to client");
        Ok(())
    }

    /// Run the connection loop until it is closed or an error occurs.
    /// This method handles incoming messages and broadcasts.
    ///
    /// # Arguments
    /// * `handler` - The message handler to process incoming messages.
    /// * `broadcast_rx` - The broadcast receiver to listen for server-wide broadcast messages.
    #[tracing::instrument(skip(self, handler), fields(client_id = self.id))]
    pub async fn run_to_completion(
        &self,
        handler: Arc<dyn MessageHandler<C, Req, Res>>,
        mut broadcast_rx: BroadcastReceiver<BroadcastMessage>,
    ) {
        let connection_id = self.id;

        loop {
            tokio::select! {
                Ok(broadcast_message) = broadcast_rx.recv() => {
                    let broadcast_message = MessageToClient::from(broadcast_message);
                    let request_id = broadcast_message.request_id();
                    if let Err(err) = self.send_message(&broadcast_message).await {
                        tracing::error!(
                            ?err, ?connection_id, ?request_id,
                            "Failed to send broadcast message to client"
                        );
                        continue;
                    }
                }

                incoming_message_result = self.read_message() => {
                    let incoming_message = match incoming_message_result {
                        Ok(message) => message,
                        Err(err) => {
                            tracing::error!(?err, "Failed to read incoming message from client");
                            continue;
                        },
                    };

                    let request_id = incoming_message.request_id();
                    tracing::trace!(?connection_id, ?request_id, "Received message");
                    self.handle_incoming_msg(incoming_message, handler.clone()).await;
                }
            }
        }
    }

    /// Handle incoming messages from the client connection.
    #[tracing::instrument(skip(self, handler, incoming_message), fields(client_id = self.id))]
    async fn handle_incoming_msg(
        &self,
        incoming_message: MessageToServer,
        handler: Arc<dyn MessageHandler<C, Req, Res>>,
    ) {
        let client_id = self.id;

        match incoming_message {
            MessageToServer::Subscribe { id, topic } => {
                self.subscribe_to(&topic).await;
                let msg = MessageToClient::SubscriptionAcked { id };
                if let Err(err) = self.send_message(&msg).await {
                    tracing::warn!(?err, ?client_id, "Failed to ack client subscription");
                };
            }
            MessageToServer::Unsubscribe { topic } => {
                self.unsubscribe_from(&topic).await;
                handler.on_unsubscribe(client_id, &topic).await;
            }
            MessageToServer::Request { id, data } => {
                let data = match C::deserialize(&data) {
                    Ok(data) => data,
                    Err(err) => {
                        tracing::error!(?err, ?client_id, "Failed to deserialize request data");
                        return;
                    }
                };

                if let Some(response_data) = handler.handle_request(id, data).await {
                    let response_data = match C::serialize(&response_data) {
                        Ok(data) => data,
                        Err(err) => {
                            tracing::error!(
                                ?err,
                                ?client_id,
                                "Failed to serialize server response"
                            );
                            return;
                        }
                    };

                    let response = MessageToClient::new_response(id, Arc::new(response_data));
                    if let Err(err) = self.send_message(&response).await {
                        tracing::warn!(?err, ?client_id, "Failed to send response to client");
                    };
                }
            }
            MessageToServer::Publish { topic, data } => {
                match self
                    .publish_tx
                    .send((client_id, topic.clone(), data.clone()))
                {
                    Ok(_) => handler.on_publish(&topic, data).await,
                    Err(_) => {
                        tracing::warn!(
                            ?client_id,
                            ?topic,
                            "Failed to send publish message to broadcast channel"
                        );
                    }
                };
            }
        }
    }
}
