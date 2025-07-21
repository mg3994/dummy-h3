//! WASI-compatible networking layer.

pub mod socket;
pub mod dns;

pub use socket::{UdpSocket, SocketAddr};
pub use dns::{resolve, DnsResolver};