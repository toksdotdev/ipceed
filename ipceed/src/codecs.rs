use serde::Deserialize;

use crate::IpcResult;

/// Trait for IPC codecs that handle serialization and deserialization of messages.
///
/// Implement this trait to support custom serialization formats (e.g., MessagePack, Protocol Buffers).
/// All messages exchanged between client and server are serialized using the configured codec.
///
/// # Example
///
/// ```
/// use ipceed::{{codecs::IpcCodec, IpcResult}};
/// use serde::{{Serialize, Deserialize}};
///
/// #[derive(Debug, Clone)]
/// pub struct MyCodec;
///
/// impl IpcCodec for MyCodec {{
///     fn serialize<T>(value: &T) -> IpcResult<Vec<u8>>
///     where
///         T: Serialize,
///     {{
///         // Custom serialization logic
///         Ok(serde_json::to_vec(value)?)
///     }}
///
///     fn deserialize<T>(bytes: &[u8]) -> IpcResult<T>
///     where
///         T: for<'de> Deserialize<'de>,
///     {{
///         // Custom deserialization logic
///         Ok(serde_json::from_slice(bytes)?)
///     }}
/// }}
/// ```
pub trait IpcCodec: Send + Sync {
    /// Serialize a value to bytes.
    ///
    /// This is used for both requests and responses.
    fn serialize<T>(value: &T) -> IpcResult<Vec<u8>>
    where
        T: serde::Serialize;

    /// Deserialize bytes to a value.
    ///
    /// This is used for both requests and responses.
    fn deserialize<T>(bytes: &[u8]) -> IpcResult<T>
    where
        T: for<'de> Deserialize<'de>;
}

/// Default JSON codec implementation using `serde_json`.
///
/// This is the default codec provided by the library and is suitable for most use cases.
/// For better performance or smaller message sizes, consider implementing a custom codec
/// using binary formats like MessagePack or Protocol Buffers.
///
/// # Example
///
/// ```
/// use ipceed::{{codecs::JsonCodec, client::IpcClient}};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {{
/// // Use JsonCodec with IpcClient
/// let (client, _rx) = IpcClient::<JsonCodec>::connect::<Vec<u8>>("/tmp/ipc").await?;
/// # Ok(())
/// # }}
/// ```
#[derive(Debug, Clone)]
pub struct JsonCodec;

impl IpcCodec for JsonCodec {
    fn serialize<T>(value: &T) -> IpcResult<Vec<u8>>
    where
        T: serde::Serialize,
    {
        Ok(serde_json::to_vec(value)?)
    }

    fn deserialize<T>(bytes: &[u8]) -> IpcResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        Ok(serde_json::from_slice::<T>(bytes)?)
    }
}
