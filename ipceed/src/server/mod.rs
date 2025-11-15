mod broadcast;
mod connection;
pub mod handler;

use std::ffi::OsString;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use dashmap::DashMap;
use interprocess::local_socket::GenericFilePath;
use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::ToFsName;
use interprocess::local_socket::tokio::Listener;
use interprocess::local_socket::tokio::Stream as LocalSocketStream;
use interprocess::local_socket::traits::tokio::Listener as ListenerExt;
use ipceed_types::message::BroadcastMessage;
use ipceed_types::message::MessageToClient;
use ipceed_types::primitives::ClientId;
use ipceed_types::primitives::Topic;
use tokio::sync::broadcast::Sender as BroadcastSender;
use tokio::sync::broadcast::channel as broadcast_channel;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::IpcError;
use crate::IpcResult;
use crate::check_broadcast_capacity;
use crate::codecs::IpcCodec;
pub use crate::server::broadcast::IpcBroadcastSender;
use crate::server::connection::ClientConnection;
use crate::server::handler::MessageHandler;

/// IPC server for accepting client connections and handling messages.
///
/// The server supports multiple communication patterns:
/// - Request-Response: Handle client requests and send responses
/// - Publish-Subscribe: Manage topic subscriptions and route published messages
/// - Broadcast: Send messages to all connected clients
pub struct IpcServer<C, Req, Res> {
    last_client_id: AtomicU64,
    listener: Arc<Listener>,
    message_handler: Arc<dyn MessageHandler<C, Req, Res>>,
    broadcast_tx: BroadcastSender<BroadcastMessage>,
    publish_tx: UnboundedSender<(ClientId, Topic, Arc<Vec<u8>>)>,
    publish_rx: UnboundedReceiver<(ClientId, Topic, Arc<Vec<u8>>)>,
    clients: Arc<DashMap<ClientId, Arc<ClientConnection<C, Req, Res>>>>,
    _phantom_codec: PhantomData<C>,
}

impl<C, Req, Res> IpcServer<C, Req, Res>
where
    C: IpcCodec + 'static,
    Req: for<'de> serde::Deserialize<'de> + Send + Sync + 'static,
    Res: serde::Serialize + Send + Sync + 'static,
{
    /// Create a new IPC server with the given message handler.
    ///
    /// # Arguments
    ///
    /// * `path` - File system path for the IPC socket
    /// * `handler` - Message handler implementation for processing client messages
    /// * `broadcast_capacity` - Maximum number of broadcast messages that can be queued
    ///
    /// # Errors
    ///
    /// - [`IpcError::InvalidBroadcastCapacity`] if capacity exceeds maximum
    /// - [`IpcError::Io`] if socket creation fails
    pub fn new(
        path: impl AsRef<Path>,
        handler: Arc<dyn MessageHandler<C, Req, Res>>,
        broadcast_capacity: usize,
    ) -> IpcResult<Self> {
        check_broadcast_capacity(broadcast_capacity)?;

        let path = path.as_ref();
        let name = OsString::from(&path)
            .to_os_string()
            .to_fs_name::<GenericFilePath>()
            .unwrap();

        let listener = ListenerOptions::new().name(name).create_tokio()?;
        let (broadcast_tx, _) = broadcast_channel(broadcast_capacity);
        let (publish_tx, publish_rx) = tokio::sync::mpsc::unbounded_channel();
        tracing::info!(?path, "IPC server listening on");

        Ok(Self {
            broadcast_tx,
            publish_tx,
            publish_rx,
            message_handler: handler,
            last_client_id: AtomicU64::new(1),
            listener: Arc::new(listener),
            clients: Arc::new(DashMap::new()),
            _phantom_codec: PhantomData,
        })
    }

    /// Get a broadcast sender for sending messages to all connected clients.
    ///
    /// The returned sender can be cloned and used from any task or thread.
    ///
    /// # Returns
    ///
    /// A broadcast sender that can send typed messages to all clients
    pub fn broadcaster(&self) -> IpcBroadcastSender<C> {
        IpcBroadcastSender::new(self.broadcast_tx.clone())
    }

    /// Accept a new client connection from the listener.
    ///
    /// This is an async operation that waits for a new connection.
    ///
    /// # Arguments
    ///
    /// * `listener` - The socket listener to accept connections from
    ///
    /// # Returns
    ///
    /// The client's socket stream
    ///
    /// # Errors
    ///
    /// Returns an error if accepting the connection fails
    async fn accept_connection(listener: Arc<Listener>) -> IpcResult<LocalSocketStream> {
        let socket = listener.accept().await?;
        Ok(socket)
    }

    /// Poll for incoming connections and published messages.
    ///
    /// This should be called in a loop to process server events:
    /// - Accepts new client connections
    /// - Routes published messages to subscribed clients
    ///
    /// Each call to `poll` processes exactly one event.
    ///
    /// # Errors
    ///
    /// Returns an error if accepting a connection fails
    pub async fn poll(&mut self) -> Result<(), IpcError> {
        tokio::select! {
            // Handle incoming publish messages
            Some((origin_client_id, topic, data)) = self.publish_rx.recv() => {
                tracing::debug!(?origin_client_id, ?topic, "Received publish message");
                self.publish_message_to_subscribers(origin_client_id, topic, data);
                Ok(())
            }

            // Handle incoming client connections
            socket_result = Self::accept_connection(self.listener.clone()) => {
                match socket_result {
                    Ok(socket) => Ok(self.handle_new_connection(socket).await),
                    Err(err) => {
                        tracing::error!(?err, "Failed to accept client connection");
                        Err(err)
                    }
                }
            }
        }
    }

    /// Handle a new client connection.
    ///
    /// Spawns a task to manage the client's lifecycle, including:
    /// - Processing incoming messages
    /// - Sending broadcast messages
    /// - Managing subscriptions
    ///
    /// The task automatically cleans up when the client disconnects.
    async fn handle_new_connection(&self, socket: LocalSocketStream) {
        let client_id = self.last_client_id.fetch_add(1, Ordering::AcqRel);
        let conn = Arc::new(ClientConnection::new(
            client_id,
            socket,
            self.publish_tx.clone(),
        ));
        self.clients.insert(client_id, conn.clone());
        tracing::info!(?client_id, "Client connected");

        let broadcast_rx = self.broadcast_tx.subscribe();
        let clients = self.clients.clone();
        let handler = self.message_handler.clone();

        tokio::spawn(async move {
            conn.run_to_completion(handler, broadcast_rx).await;
            // On run loop stopped, evict the connection.
            clients.remove(&client_id);
            tracing::info!(?client_id, "Client disconnected");
        });
    }

    /// Publish a message to all clients subscribed to a topic.
    ///
    /// The message is sent to all subscribers except the client that published it.
    /// This spawns a task to send messages concurrently without blocking the server loop.
    ///
    /// # Arguments
    ///
    /// * `origin_client_id` - The client that published the message (excluded from delivery)
    /// * `topic` - The topic the message was published to
    /// * `data` - The serialized message data
    fn publish_message_to_subscribers(
        &self,
        origin_client_id: ClientId,
        topic: Topic,
        data: Arc<Vec<u8>>,
    ) {
        let clients = self.clients.clone();

        // Spawn a task to send the message to all subscribed clients
        // This allows us to avoid blocking the main server loop while sending messages
        // and ensures that we can handle multiple publishes concurrently.
        //
        // Future improvement to guarantee every client gets the message at roughly
        // the same time could be to switch each client having it's own broadcast receiver,
        // and here we write the published message to the broadcast sender. Furthermore,
        // each client could then keep a backlog of it's own un-acked message, and resent at
        // a later time.
        tokio::spawn(async move {
            for client_ref in clients.iter() {
                let client = client_ref.value();

                if client.id() != origin_client_id && client.is_subscribed_to(&topic).await {
                    let msg = MessageToClient::new_published(topic.clone(), data.clone());

                    if let Err(err) = client.send_message(&msg).await {
                        tracing::warn!(
                            ?err,
                            key = ?client_ref.key(),
                            "Failed to send publish to client"
                        );
                    }
                }
            }
        });
    }
}
