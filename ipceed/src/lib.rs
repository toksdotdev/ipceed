//! Async, cross-platform inter-process communication (IPC) library with multiple messaging patterns.
//!
//! # Features
//!
//! - **Request-Response**: Send requests and receive typed responses with timeout support
//! - **Server Broadcast**: Send messages from server to all connected clients
//! - **Publish-Subscribe**: Topic-based messaging with subscription management
//! - **Type-safe**: Generic codec system for serialization (JSON by default)
//! - **Async**: Built on Tokio for high-performance non-blocking I/O
//!
//! # Example
//!
//! ```no_run
//! use ipceed::{{server::IpcServer, client::IpcClient, codecs::JsonCodec}};
//! use std::sync::Arc;
//!
//! // Define your message types
//! # use serde::{{Serialize, Deserialize}};
//! #[derive(Serialize, Deserialize)]
//! struct Request {{ message: String }}
//! #[derive(Serialize, Deserialize)]
//! struct Response {{ reply: String }}
//! ```

use thiserror::Error;

pub mod client;
pub mod codecs;
pub mod server;

#[cfg(test)]
mod tests;

/// Maximum allowed capacity for broadcast channels to prevent overflow.
pub(crate) const MAX_BROADCAST_CAPACITY: usize = usize::MAX / 2;

/// Result type alias for IPC operations.
pub type IpcResult<T> = std::result::Result<T, IpcError>;

/// Errors that can occur during IPC operations.
#[derive(Debug, Error)]
pub enum IpcError {
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Timeout error")]
    Timeout,

    /// Broadcast error, also including the data that couldn't be broadcasted.
    #[error("Broadcast failed")]
    BroadcastFailed,

    /// Request failed (e.g., channel closed before response received).
    #[error("Request failed")]
    RequestFailed,

    /// Request ID not found in pending requests map.
    #[error("Request not found: {0}")]
    RequestNotFound(String),

    #[error("Message handler error: {0}")]
    TokioJoinError(#[from] tokio::task::JoinError),

    #[error("Broadcast capacity cannot be more than {MAX_BROADCAST_CAPACITY}")]
    InvalidBroadcastCapacity,
}

/// Validates that the broadcast capacity does not exceed the maximum allowed value.
///
/// # Errors
///
/// Returns [`IpcError::InvalidBroadcastCapacity`] if capacity exceeds [`MAX_BROADCAST_CAPACITY`].
pub(crate) fn check_broadcast_capacity(capacity: usize) -> IpcResult<()> {
    if capacity > MAX_BROADCAST_CAPACITY {
        return Err(IpcError::InvalidBroadcastCapacity);
    }

    Ok(())
}
