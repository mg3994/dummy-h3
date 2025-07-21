//! QUIC connection management.
//!
//! This module provides QUIC connection handling with WASI compatibility.

use crate::crypto::protection::PacketProtection;
use crate::crypto::tls::{TlsConfig, TlsSession};
use crate::error::{ConnectionError, H3Result};
use crate::network::socket::UdpSocket;
use crate::protocol::StreamType;
use crate::transport::QuicStream;
use crate::transport::packet::Packet;
use bytes::{Bytes, BytesMut};
use rand::Rng;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Maximum QUIC packet size.
pub const MAX_PACKET_SIZE: usize = 1350;

/// Configuration for QUIC connections.
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Connection timeout
    pub timeout: Duration,
    /// Idle timeout
    pub idle_timeout: Duration,
    /// Keep-alive interval
    pub keep_alive: Option<Duration>,
    /// Maximum concurrent streams
    pub max_streams: u64,
    /// Initial flow control window size
    pub initial_window_size: u64,
    /// Maximum packet size
    pub max_packet_size: usize,
    /// Stream configuration
    pub stream_config: StreamConfig,
    /// TLS configuration
    pub tls_config: Arc<TlsConfig>,
}

/// Stream configuration.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Initial flow control window size for streams
    pub initial_window_size: u32,
    /// Default stream priority
    pub priority: crate::protocol::stream::StreamPriority,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            initial_window_size: 65536,
            priority: crate::protocol::stream::StreamPriority::Normal,
        }
    }
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(30),
            keep_alive: Some(Duration::from_secs(15)),
            max_streams: 100,
            initial_window_size: 65536,
            max_packet_size: MAX_PACKET_SIZE,
            stream_config: StreamConfig::default(),
            tls_config: Arc::new(TlsConfig::new()),
        }
    }
}

/// QUIC connection identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionId(pub [u8; 16]);

impl ConnectionId {
    /// Generate a new random connection ID.
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let mut id = [0u8; 16];
        rng.fill(&mut id);
        Self(id)
    }

    /// Create a connection ID from a byte slice.
    pub fn from_slice(slice: &[u8]) -> Option<Self> {
        if slice.len() >= 16 {
            let mut id = [0u8; 16];
            id.copy_from_slice(&slice[..16]);
            Some(Self(id))
        } else {
            None
        }
    }

    /// Get the raw bytes of the connection ID.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// QUIC packet types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// Initial packet
    Initial,
    /// 0-RTT packet
    ZeroRTT,
    /// Handshake packet
    Handshake,
    /// Retry packet
    Retry,
    /// 1-RTT packet
    OneRTT,
    /// Version negotiation packet
    VersionNegotiation,
}

/// Connection state tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Connection is being established
    Connecting,
    /// Connection is established and active
    Connected,
    /// Connection is being closed gracefully
    Closing,
    /// Connection is closed
    Closed,
    /// Connection encountered an error
    Error,
}

/// A QUIC connection for HTTP/3 transport.
#[derive(Debug)]
pub struct QuicConnection {
    /// Connection ID
    pub id: ConnectionId,
    /// Remote address
    pub remote_addr: SocketAddr,
    /// Connection configuration
    pub config: ConnectionConfig,
    /// Connection state
    pub state: ConnectionState,
    /// UDP socket for sending/receiving packets
    socket: Arc<UdpSocket>,
    /// Stream manager
    stream_manager: crate::protocol::stream::StreamManager,
    /// Connection statistics
    stats: ConnectionStats,
    /// Connection creation time
    created_at: Instant,
    /// Last activity time
    last_activity: Instant,
    /// Packet buffer for receiving
    recv_buffer: BytesMut,
    /// Next packet number for sending
    next_packet_number: u64,
    /// Received packet numbers to detect duplicates
    received_packet_numbers: HashMap<u64, Instant>,
    /// Pending outgoing packets
    outgoing_packets: VecDeque<Packet>,
    /// Pending incoming packets
    incoming_packets: VecDeque<Packet>,
    /// Whether the connection is initiated by us
    is_client: bool,
    /// Peer's connection ID
    peer_connection_id: Option<ConnectionId>,
    /// Local connection ID
    local_connection_id: ConnectionId,
    /// TLS session
    tls_session: Option<TlsSession>,
    /// Packet protection
    packet_protection: Option<PacketProtection>,
}

impl QuicConnection {
    /// Create a new QUIC connection.
    pub async fn new(
        socket: Arc<UdpSocket>,
        remote_addr: SocketAddr,
        config: ConnectionConfig,
        is_client: bool,
    ) -> H3Result<Self> {
        let local_connection_id = ConnectionId::new();
        let id = local_connection_id;
        let now = Instant::now();

        // Convert our StreamConfig to the protocol module's StreamConfig
        let protocol_stream_config = crate::config::StreamConfig {
            initial_window_size: config.stream_config.initial_window_size,
            max_frame_size: 16384, // Default value
            priority: config.stream_config.priority,
            enable_flow_control: true,
        };

        let stream_manager =
            crate::protocol::stream::StreamManager::new(protocol_stream_config, is_client);

        let mut connection = Self {
            id,
            remote_addr,
            config: config.clone(),
            state: ConnectionState::Connecting,
            socket,
            stream_manager,
            stats: ConnectionStats::default(),
            created_at: now,
            last_activity: now,
            recv_buffer: BytesMut::with_capacity(MAX_PACKET_SIZE),
            next_packet_number: 0,
            received_packet_numbers: HashMap::new(),
            outgoing_packets: VecDeque::new(),
            incoming_packets: VecDeque::new(),
            is_client,
            peer_connection_id: None,
            local_connection_id,
            tls_session: None,
            packet_protection: None,
        };

        // Initialize TLS if we're a client
        if is_client {
            let server_name = remote_addr.ip().to_string();
            let tls_session = TlsSession::new_client(config.tls_config.clone(), &server_name)?;
            let packet_protection =
                PacketProtection::new(tls_session, connection.local_connection_id, true)?;
            connection.packet_protection = Some(packet_protection);
        }

        // If client, initiate the connection
        if is_client {
            connection.initiate_connection().await?;
        }

        Ok(connection)
    }

    /// Establish a new QUIC connection.
    pub async fn connect(addr: SocketAddr, config: ConnectionConfig) -> H3Result<Self> {
        tracing::info!("Establishing QUIC connection to {}", addr);

        // Create a UDP socket
        let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);

        // Create and initialize the connection
        let connection = Self::new(socket, addr, config, true).await?;

        Ok(connection)
    }

    /// Initiate a connection to the remote peer.
    async fn initiate_connection(&mut self) -> H3Result<()> {
        tracing::debug!("Initiating QUIC connection to {}", self.remote_addr);

        // Create an Initial packet
        let packet = self.create_initial_packet()?;

        // Send the Initial packet
        self.send_packet(&packet).await?;

        Ok(())
    }

    /// Create an Initial packet for connection establishment.
    fn create_initial_packet(&self) -> H3Result<Bytes> {
        // In a real implementation, this would create a proper QUIC Initial packet
        // For now, we'll just create a simple placeholder packet
        let mut buffer = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // Add packet header (simplified)
        buffer.extend_from_slice(&[0x80]); // Initial packet type
        buffer.extend_from_slice(&self.local_connection_id.0); // Connection ID
        buffer.extend_from_slice(&[0, 0, 0, 0]); // Packet number (simplified)

        // Add some payload (simplified)
        buffer.extend_from_slice(b"QUIC Initial Packet");

        Ok(buffer.freeze())
    }

    /// Send a packet to the remote peer.
    async fn send_packet(&mut self, packet: &Bytes) -> H3Result<()> {
        tracing::trace!("Sending {} bytes to {}", packet.len(), self.remote_addr);

        let bytes_sent = self.socket.send_to(packet, self.remote_addr).await?;

        // Update statistics
        self.stats.packets_sent += 1;
        self.stats.bytes_sent += bytes_sent as u64;
        self.last_activity = Instant::now();

        // Increment packet number
        self.next_packet_number += 1;

        Ok(())
    }

    /// Process incoming packets.
    pub async fn process_incoming(&mut self) -> H3Result<()> {
        tracing::trace!("Processing incoming packets");

        // Clear the receive buffer
        self.recv_buffer.clear();

        // Receive a packet
        let (bytes_read, from_addr) = self.socket.recv_from(&mut self.recv_buffer).await?;

        // Check if we received anything
        if bytes_read == 0 {
            return Ok(());
        }

        // Check if the packet is from the expected peer
        if from_addr != self.remote_addr {
            tracing::warn!("Received packet from unexpected address: {}", from_addr);
            return Ok(());
        }

        // Update statistics
        self.stats.packets_received += 1;
        self.stats.bytes_received += bytes_read as u64;
        self.last_activity = Instant::now();

        // Process the packet
        let packet_data = self.recv_buffer[..bytes_read].to_vec();
        self.process_packet(&packet_data).await?;

        Ok(())
    }

    /// Process a received packet.
    async fn process_packet(&mut self, packet: &[u8]) -> H3Result<()> {
        tracing::trace!("Processing packet of {} bytes", packet.len());

        // In a real implementation, this would parse and process the QUIC packet
        // For now, we'll just handle some basic packet types
        if packet.is_empty() {
            return Ok(());
        }

        // Check packet type (simplified)
        match packet[0] {
            0x80 => self.handle_initial_packet(packet).await?,
            0x81 => self.handle_handshake_packet(packet).await?,
            0x82 => self.handle_data_packet(packet).await?,
            _ => tracing::warn!("Unknown packet type: {}", packet[0]),
        }

        Ok(())
    }

    /// Handle an Initial packet.
    async fn handle_initial_packet(&mut self, _packet: &[u8]) -> H3Result<()> {
        tracing::debug!("Handling Initial packet");

        // In a real implementation, this would handle the QUIC Initial packet
        // For now, we'll just update the connection state
        if self.state == ConnectionState::Connecting {
            // If we're a server, send a response
            if !self.is_client {
                let response = self.create_handshake_packet()?;
                self.send_packet(&response).await?;
            }
        }

        Ok(())
    }

    /// Handle a Handshake packet.
    async fn handle_handshake_packet(&mut self, _packet: &[u8]) -> H3Result<()> {
        tracing::debug!("Handling Handshake packet");

        // In a real implementation, this would handle the QUIC Handshake packet
        // For now, we'll just update the connection state
        if self.state == ConnectionState::Connecting {
            self.state = ConnectionState::Connected;

            // Record establishment time
            self.stats.establishment_time = Some(self.created_at.elapsed());

            tracing::info!("QUIC connection established with {}", self.remote_addr);

            // If we're a client, send an acknowledgment
            if self.is_client {
                let ack = self.create_data_packet()?;
                self.send_packet(&ack).await?;
            }
        }

        Ok(())
    }

    /// Handle a Data packet.
    async fn handle_data_packet(&mut self, _packet: &[u8]) -> H3Result<()> {
        tracing::trace!("Handling Data packet");

        // In a real implementation, this would handle the QUIC Data packet
        // For now, we'll just update the connection state
        if self.state == ConnectionState::Connected {
            // Process the data (simplified)
            // In a real implementation, this would extract stream data and process it

            // Send an acknowledgment (simplified)
            let ack = self.create_ack_packet()?;
            self.send_packet(&ack).await?;
        }

        Ok(())
    }

    /// Create a Handshake packet.
    fn create_handshake_packet(&self) -> H3Result<Bytes> {
        // In a real implementation, this would create a proper QUIC Handshake packet
        // For now, we'll just create a simple placeholder packet
        let mut buffer = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // Add packet header (simplified)
        buffer.extend_from_slice(&[0x81]); // Handshake packet type
        buffer.extend_from_slice(&self.local_connection_id.0); // Connection ID
        buffer.extend_from_slice(&[0, 0, 0, 0]); // Packet number (simplified)

        // Add some payload (simplified)
        buffer.extend_from_slice(b"QUIC Handshake Packet");

        Ok(buffer.freeze())
    }

    /// Create a Data packet.
    fn create_data_packet(&self) -> H3Result<Bytes> {
        // In a real implementation, this would create a proper QUIC Data packet
        // For now, we'll just create a simple placeholder packet
        let mut buffer = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // Add packet header (simplified)
        buffer.extend_from_slice(&[0x82]); // Data packet type
        buffer.extend_from_slice(&self.local_connection_id.0); // Connection ID
        buffer.extend_from_slice(&[0, 0, 0, 0]); // Packet number (simplified)

        // Add some payload (simplified)
        buffer.extend_from_slice(b"QUIC Data Packet");

        Ok(buffer.freeze())
    }

    /// Create an ACK packet.
    fn create_ack_packet(&self) -> H3Result<Bytes> {
        // In a real implementation, this would create a proper QUIC ACK packet
        // For now, we'll just create a simple placeholder packet
        let mut buffer = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // Add packet header (simplified)
        buffer.extend_from_slice(&[0x83]); // ACK packet type
        buffer.extend_from_slice(&self.local_connection_id.0); // Connection ID
        buffer.extend_from_slice(&[0, 0, 0, 0]); // Packet number (simplified)

        // Add some payload (simplified)
        buffer.extend_from_slice(b"QUIC ACK Packet");

        Ok(buffer.freeze())
    }

    /// Accept an incoming stream.
    pub async fn accept_stream(&mut self) -> H3Result<QuicStream> {
        if self.state != ConnectionState::Connected {
            return Err(ConnectionError::InvalidState(format!(
                "Cannot accept stream in state: {:?}",
                self.state
            ))
            .into());
        }

        // Process any pending packets to see if there are new streams
        self.process_incoming().await?;

        // In a real implementation, this would wait for a stream to be opened by the peer
        // For now, we'll just create a placeholder stream
        tracing::debug!("Accepting incoming stream");

        // Generate a stream ID based on whether we're client or server
        let stream_id = if self.is_client {
            // Server-initiated streams have LSB 1
            1
        } else {
            // Client-initiated streams have LSB 0
            0
        };

        Ok(QuicStream::new(stream_id, true))
    }

    /// Open a new outgoing stream.
    pub async fn open_stream(&mut self, stream_type: StreamType) -> H3Result<QuicStream> {
        if self.state != ConnectionState::Connected {
            return Err(ConnectionError::InvalidState(format!(
                "Cannot open stream in state: {:?}",
                self.state
            ))
            .into());
        }

        tracing::debug!("Opening new {:?} stream", stream_type);

        // Create the stream in the stream manager
        let stream_id = self.stream_manager.create_stream(stream_type)?;

        let is_bidirectional = stream_type.is_bidirectional();
        Ok(QuicStream::new(stream_id, is_bidirectional))
    }

    /// Send data on a stream.
    pub async fn send_stream_data(
        &mut self,
        stream_id: u64,
        data: &[u8],
        fin: bool,
    ) -> H3Result<usize> {
        tracing::trace!("Sending {} bytes on stream {}", data.len(), stream_id);

        // Check if the connection is established
        if self.state != ConnectionState::Connected {
            return Err(ConnectionError::InvalidState(format!(
                "Cannot send stream data in state: {:?}",
                self.state
            ))
            .into());
        }

        // In a real implementation, this would fragment the data into packets if needed
        // For now, we'll just create a single packet with the data

        // Create a stream data packet
        let mut buffer = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // Add packet header (simplified)
        buffer.extend_from_slice(&[0x82]); // Data packet type
        buffer.extend_from_slice(&self.local_connection_id.0); // Connection ID
        buffer.extend_from_slice(&[0, 0, 0, 0]); // Packet number (simplified)

        // Add stream ID and data (simplified)
        buffer.extend_from_slice(&stream_id.to_be_bytes());
        buffer.extend_from_slice(data);

        // Add FIN flag if needed
        if fin {
            buffer.extend_from_slice(&[0x01]);
        } else {
            buffer.extend_from_slice(&[0x00]);
        }

        // Send the packet
        self.send_packet(&buffer.freeze()).await?;

        Ok(data.len())
    }

    /// Send a datagram.
    pub async fn send_datagram(&mut self, data: &[u8]) -> H3Result<()> {
        if self.state != ConnectionState::Connected {
            return Err(ConnectionError::InvalidState(format!(
                "Cannot send datagram in state: {:?}",
                self.state
            ))
            .into());
        }

        tracing::debug!("Sending datagram of {} bytes", data.len());

        // Create a datagram packet
        let mut buffer = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // Add packet header (simplified)
        buffer.extend_from_slice(&[0x84]); // Datagram packet type
        buffer.extend_from_slice(&self.local_connection_id.0); // Connection ID
        buffer.extend_from_slice(&[0, 0, 0, 0]); // Packet number (simplified)

        // Add datagram data
        buffer.extend_from_slice(data);

        // Send the packet
        self.send_packet(&buffer.freeze()).await?;

        Ok(())
    }

    /// Receive a datagram.
    pub async fn recv_datagram(&mut self) -> H3Result<Option<Bytes>> {
        if self.state != ConnectionState::Connected {
            return Err(ConnectionError::InvalidState(format!(
                "Cannot receive datagram in state: {:?}",
                self.state
            ))
            .into());
        }

        // Process any pending packets
        self.process_incoming().await?;

        // In a real implementation, this would extract datagrams from received packets
        // For now, we'll just return None
        Ok(None)
    }

    /// Close the connection gracefully.
    pub async fn close(&mut self) -> H3Result<()> {
        tracing::info!("Closing QUIC connection to {}", self.remote_addr);

        // Update state
        self.state = ConnectionState::Closing;

        // Send a CONNECTION_CLOSE frame (simplified)
        let close_packet = self.create_close_packet(0, "Normal closure")?;
        self.send_packet(&close_packet).await?;

        // Update state
        self.state = ConnectionState::Closed;

        Ok(())
    }

    /// Create a CONNECTION_CLOSE packet.
    fn create_close_packet(&self, error_code: u16, reason: &str) -> H3Result<Bytes> {
        // In a real implementation, this would create a proper QUIC CONNECTION_CLOSE packet
        // For now, we'll just create a simple placeholder packet
        let mut buffer = BytesMut::with_capacity(MAX_PACKET_SIZE);

        // Add packet header (simplified)
        buffer.extend_from_slice(&[0x85]); // CONNECTION_CLOSE packet type
        buffer.extend_from_slice(&self.local_connection_id.0); // Connection ID
        buffer.extend_from_slice(&[0, 0, 0, 0]); // Packet number (simplified)

        // Add error code and reason (simplified)
        buffer.extend_from_slice(&error_code.to_be_bytes());
        buffer.extend_from_slice(reason.as_bytes());

        Ok(buffer.freeze())
    }

    /// Get the remote address.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// Check if the connection is active.
    pub fn is_connected(&self) -> bool {
        self.state == ConnectionState::Connected
    }

    /// Check if the connection is alive.
    pub fn is_alive(&self) -> bool {
        matches!(
            self.state,
            ConnectionState::Connecting | ConnectionState::Connected
        )
    }

    /// Check if the connection has timed out.
    pub fn is_timed_out(&self) -> bool {
        self.last_activity.elapsed() > self.config.idle_timeout
    }

    /// Get connection statistics.
    pub fn stats(&self) -> &ConnectionStats {
        &self.stats
    }

    /// Get the stream manager.
    pub fn stream_manager(&mut self) -> &mut crate::protocol::stream::StreamManager {
        &mut self.stream_manager
    }

    /// Perform connection maintenance.
    pub async fn maintain(&mut self) -> H3Result<()> {
        // Check for timeout
        if self.is_timed_out() {
            tracing::info!("Connection to {} timed out", self.remote_addr);
            self.state = ConnectionState::Closed;
            return Ok(());
        }

        // Send keep-alive if needed
        if let Some(keep_alive) = self.config.keep_alive {
            if self.last_activity.elapsed() > keep_alive && self.is_connected() {
                tracing::debug!("Sending keep-alive packet");
                let keep_alive_packet = self.create_data_packet()?;
                self.send_packet(&keep_alive_packet).await?;
            }
        }

        // Process any incoming packets
        self.process_incoming().await?;

        Ok(())
    }
}

/// Connection statistics.
#[derive(Debug, Clone, Default)]
pub struct ConnectionStats {
    /// Total bytes sent
    pub bytes_sent: u64,
    /// Total bytes received
    pub bytes_received: u64,
    /// Total packets sent
    pub packets_sent: u64,
    /// Total packets received
    pub packets_received: u64,
    /// Round-trip time
    pub rtt: Duration,
    /// Connection establishment time
    pub establishment_time: Option<Duration>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connection_creation() {
        let config = ConnectionConfig::default();
        let addr = "127.0.0.1:8080".parse().unwrap();

        // This would fail in a real implementation without a server
        // but our stub implementation should succeed
        let result = QuicConnection::connect(addr, config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_connection_id_generation() {
        let id1 = ConnectionId::new();
        let id2 = ConnectionId::new();

        // Two randomly generated IDs should be different
        assert_ne!(id1, id2);

        // Test from_slice
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let id = ConnectionId::from_slice(&bytes).unwrap();
        assert_eq!(id.0, bytes);

        // Test as_bytes
        assert_eq!(id.as_bytes(), &bytes);
    }

    #[tokio::test]
    async fn test_connection_state_transitions() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let remote_addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let config = ConnectionConfig::default();

        let mut connection = QuicConnection::new(socket, remote_addr, config, true)
            .await
            .unwrap();
        assert_eq!(connection.state, ConnectionState::Connecting);

        // Simulate receiving a handshake packet
        let handshake_packet = [
            0x81, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        connection.process_packet(&handshake_packet).await.unwrap();
        assert_eq!(connection.state, ConnectionState::Connected);

        // Close the connection
        connection.close().await.unwrap();
        assert_eq!(connection.state, ConnectionState::Closed);
        assert!(!connection.is_alive());
    }

    #[tokio::test]
    async fn test_connection_timeout() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let remote_addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let mut config = ConnectionConfig::default();
        config.idle_timeout = Duration::from_millis(1);

        let mut connection = QuicConnection::new(socket, remote_addr, config, true)
            .await
            .unwrap();

        // Sleep to trigger timeout
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(connection.is_timed_out());

        // Maintain should close the connection due to timeout
        connection.maintain().await.unwrap();
        assert_eq!(connection.state, ConnectionState::Closed);
    }
}
