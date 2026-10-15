//! Transport-neutral App Server connection and versioned JSON-RPC contract.

mod admission;
pub(crate) use admission::RequestLane;
pub mod connection;
pub mod editor_read;
pub mod protocol;
pub(crate) mod registration;
pub mod schema;
pub mod session_deletion;
mod wire;

pub mod process;
pub mod transport;

pub mod host;

pub mod archive_download;

pub(crate) mod delivery_access;
pub(crate) mod product_host;

pub mod upload_transfer;
