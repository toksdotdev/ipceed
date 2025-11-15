//! Shared types for IPC message protocol and primitive types.
//!
//! This crate contains the core message types and primitives used for communication
//! between IPC clients and servers. These types are shared across the main `ipceed` crate.
//!
//! # Main types
//!
//! - [`message::MessageToServer`]: Messages sent from client to server
//! - [`message::MessageToClient`]: Messages sent from server to client
//! - [`primitives::RequestId`]: Unique identifier for requests
//! - [`primitives::Topic`]: Topic identifier for pub/sub messaging
//! - [`primitives::ClientId`]: Unique identifier for connected clients

pub mod message;
pub mod primitives;
