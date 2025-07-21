//! QUIC transport layer implementation.

pub mod connection;
pub mod endpoint;
pub mod packet;

pub use connection::{QuicConnection, ConnectionConfig, ConnectionId, PacketType};
pub use endpoint::{Endpoint, QuicListener};
pub use packet::{Packet, PacketHeader, AckFrame, AckTracker, LossDetector};

/// QUIC stream representation.
#[derive(Debug)]
pub struct QuicStream {
    /// Stream identifier
    pub id: u64,
    /// Whether the stream is bidirectional
    pub is_bidirectional: bool,
}

impl QuicStream {
    /// Create a new QUIC stream.
    pub fn new(id: u64, is_bidirectional: bool) -> Self {
        Self { id, is_bidirectional }
    }
    
    /// Send data on this stream.
    pub async fn send(&mut self, data: &[u8]) -> crate::error::H3Result<()> {
        // TODO: Implement actual sending
        tracing::debug!("Sending {} bytes on stream {}", data.len(), self.id);
        Ok(())
    }
    
    /// Receive data from this stream.
    pub async fn recv(&mut self) -> crate::error::H3Result<Option<bytes::Bytes>> {
        // TODO: Implement actual receiving
        Ok(None)
    }
    
    /// Close the stream.
    pub async fn close(&mut self) -> crate::error::H3Result<()> {
        tracing::debug!("Closing stream {}", self.id);
        Ok(())
    }
}