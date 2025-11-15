use std::ops::Deref;

use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

/// Unique identifier for a connected client.
///
/// This is assigned by the server when a client connects and remains
/// constant for the lifetime of the connection.
pub type ClientId = u64;

/// Unique identifier for requests and responses.
///
/// Generated using UUID v4 to ensure uniqueness across all clients and requests.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(Uuid);

impl Deref for RequestId {
    type Target = Uuid;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl RequestId {
    /// Generate a new unique request ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Topic identifier for publish-subscribe messaging.
///
/// Topics are used to route published messages to interested subscribers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Topic(String);

impl Deref for Topic {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Topic {
    /// Create a new topic from a string.
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic name
    pub fn new<S: Into<String>>(topic: S) -> Self {
        Self(topic.into())
    }
}

impl<T: AsRef<str>> From<T> for Topic {
    fn from(topic: T) -> Self {
        Self(topic.as_ref().to_string())
    }
}
