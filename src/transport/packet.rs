//! QUIC packet handling and reliability.
//!
//! This module provides packet parsing, generation, acknowledgment handling,
//! and loss detection for QUIC connections.

use crate::error::{H3Result, TransportError};
use crate::transport::ConnectionId;
use bytes::{Bytes, BytesMut, Buf, BufMut};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Maximum QUIC packet size.
pub const MAX_PACKET_SIZE: usize = 1350;

/// QUIC packet types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// Initial packet
    Initial = 0x80,
    /// 0-RTT packet
    ZeroRTT = 0x81,
    /// Handshake packet
    Handshake = 0x82,
    /// Retry packet
    Retry = 0x83,
    /// 1-RTT packet (short header)
    OneRTT = 0x40,
    /// Version negotiation packet
    VersionNegotiation = 0x00,
}

impl PacketType {
    /// Parse packet type from the first byte.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte & 0xF0 {
            0x80 => Some(PacketType::Initial),
            0x90 => Some(PacketType::ZeroRTT),
            0xA0 => Some(PacketType::Handshake),
            0xB0 => Some(PacketType::Retry),
            0x40..=0x7F => Some(PacketType::OneRTT),
            0x00 => Some(PacketType::VersionNegotiation),
            _ => None,
        }
    }
}

/// QUIC packet header.
#[derive(Debug, Clone)]
pub struct PacketHeader {
    /// Packet type
    pub packet_type: PacketType,
    /// Connection ID
    pub connection_id: ConnectionId,
    /// Packet number
    pub packet_number: u64,
    /// Version (for long header packets)
    pub version: Option<u32>,
    /// Source connection ID (for long header packets)
    pub source_connection_id: Option<ConnectionId>,
    /// Destination connection ID (for long header packets)
    pub destination_connection_id: Option<ConnectionId>,
}

impl PacketHeader {
    /// Create a new packet header.
    pub fn new(
        packet_type: PacketType,
        connection_id: ConnectionId,
        packet_number: u64,
    ) -> Self {
        Self {
            packet_type,
            connection_id,
            packet_number,
            version: None,
            source_connection_id: None,
            destination_connection_id: None,
        }
    }
    
    /// Encode the packet header to bytes.
    pub fn encode(&self) -> H3Result<Bytes> {
        let mut buffer = BytesMut::with_capacity(64);
        
        // Write packet type
        buffer.put_u8(self.packet_type as u8);
        
        // For long header packets, include version and connection IDs
        match self.packet_type {
            PacketType::Initial | PacketType::ZeroRTT | PacketType::Handshake | PacketType::Retry => {
                // Version
                buffer.put_u32(self.version.unwrap_or(1));
                
                // Destination connection ID length and value
                buffer.put_u8(16); // Connection ID length
                buffer.extend_from_slice(&self.connection_id.0);
                
                // Source connection ID length and value
                if let Some(src_id) = &self.source_connection_id {
                    buffer.put_u8(16);
                    buffer.extend_from_slice(&src_id.0);
                } else {
                    buffer.put_u8(0);
                }
            }
            PacketType::OneRTT => {
                // Short header - just connection ID
                buffer.extend_from_slice(&self.connection_id.0);
            }
            PacketType::VersionNegotiation => {
                // Version negotiation packet format
                buffer.put_u32(0); // Version 0 indicates version negotiation
                buffer.put_u8(16);
                buffer.extend_from_slice(&self.connection_id.0);
            }
        }
        
        // Packet number (simplified - in real implementation this would be variable length)
        buffer.put_u32(self.packet_number as u32);
        
        Ok(buffer.freeze())
    }
    
    /// Decode a packet header from bytes.
    pub fn decode(mut data: &[u8]) -> H3Result<(Self, usize)> {
        if data.is_empty() {
            return Err(TransportError::InvalidPacket("Empty packet".to_string()).into());
        }
        
        let original_len = data.len();
        let first_byte = data.get_u8();
        
        let packet_type = PacketType::from_byte(first_byte)
            .ok_or_else(|| TransportError::InvalidPacket("Invalid packet type".to_string()))?;
        
        let mut header = PacketHeader {
            packet_type,
            connection_id: ConnectionId([0; 16]),
            packet_number: 0,
            version: None,
            source_connection_id: None,
            destination_connection_id: None,
        };
        
        match packet_type {
            PacketType::Initial | PacketType::ZeroRTT | PacketType::Handshake | PacketType::Retry => {
                // Long header packet
                if data.remaining() < 4 {
                    return Err(TransportError::InvalidPacket("Incomplete version".to_string()).into());
                }
                header.version = Some(data.get_u32());
                
                // Destination connection ID
                if data.remaining() < 1 {
                    return Err(TransportError::InvalidPacket("Missing DCID length".to_string()).into());
                }
                let dcid_len = data.get_u8() as usize;
                if dcid_len > 0 {
                    if data.remaining() < dcid_len {
                        return Err(TransportError::InvalidPacket("Incomplete DCID".to_string()).into());
                    }
                    let mut dcid = [0u8; 16];
                    if dcid_len <= 16 {
                        data.copy_to_slice(&mut dcid[..dcid_len]);
                        header.connection_id = ConnectionId(dcid);
                    }
                }
                
                // Source connection ID
                if data.remaining() < 1 {
                    return Err(TransportError::InvalidPacket("Missing SCID length".to_string()).into());
                }
                let scid_len = data.get_u8() as usize;
                if scid_len > 0 {
                    if data.remaining() < scid_len {
                        return Err(TransportError::InvalidPacket("Incomplete SCID".to_string()).into());
                    }
                    let mut scid = [0u8; 16];
                    if scid_len <= 16 {
                        data.copy_to_slice(&mut scid[..scid_len]);
                        header.source_connection_id = Some(ConnectionId(scid));
                    }
                }
            }
            PacketType::OneRTT => {
                // Short header packet
                if data.remaining() < 16 {
                    return Err(TransportError::InvalidPacket("Incomplete connection ID".to_string()).into());
                }
                let mut cid = [0u8; 16];
                data.copy_to_slice(&mut cid);
                header.connection_id = ConnectionId(cid);
            }
            PacketType::VersionNegotiation => {
                // Version negotiation
                if data.remaining() < 4 {
                    return Err(TransportError::InvalidPacket("Incomplete version".to_string()).into());
                }
                header.version = Some(data.get_u32());
                
                if data.remaining() < 1 {
                    return Err(TransportError::InvalidPacket("Missing DCID length".to_string()).into());
                }
                let dcid_len = data.get_u8() as usize;
                if dcid_len > 0 && data.remaining() >= dcid_len {
                    let mut dcid = [0u8; 16];
                    if dcid_len <= 16 {
                        data.copy_to_slice(&mut dcid[..dcid_len]);
                        header.connection_id = ConnectionId(dcid);
                    }
                }
            }
        }
        
        // Packet number (simplified)
        if data.remaining() >= 4 {
            header.packet_number = data.get_u32() as u64;
        }
        
        let consumed = original_len - data.remaining();
        Ok((header, consumed))
    }
}

/// QUIC packet.
#[derive(Debug, Clone)]
pub struct Packet {
    /// Packet header
    pub header: PacketHeader,
    /// Packet payload
    pub payload: Bytes,
    /// Timestamp when packet was created/received
    pub timestamp: Instant,
    /// Whether this packet needs acknowledgment
    pub ack_eliciting: bool,
}

impl Packet {
    /// Create a new packet.
    pub fn new(
        packet_type: PacketType,
        connection_id: ConnectionId,
        packet_number: u64,
        payload: Bytes,
    ) -> Self {
        let header = PacketHeader::new(packet_type, connection_id, packet_number);
        Self {
            header,
            payload,
            timestamp: Instant::now(),
            ack_eliciting: true,
        }
    }
    
    /// Encode the packet to bytes.
    pub fn encode(&self) -> H3Result<Bytes> {
        let header_bytes = self.header.encode()?;
        let mut buffer = BytesMut::with_capacity(header_bytes.len() + self.payload.len());
        
        buffer.extend_from_slice(&header_bytes);
        buffer.extend_from_slice(&self.payload);
        
        Ok(buffer.freeze())
    }
    
    /// Decode a packet from bytes.
    pub fn decode(data: &[u8]) -> H3Result<Self> {
        let (header, header_len) = PacketHeader::decode(data)?;
        
        if data.len() < header_len {
            return Err(TransportError::InvalidPacket("Packet too short".to_string()).into());
        }
        
        let payload = Bytes::copy_from_slice(&data[header_len..]);
        
        Ok(Self {
            header,
            payload,
            timestamp: Instant::now(),
            ack_eliciting: true,
        })
    }
    
    /// Get the packet size.
    pub fn size(&self) -> usize {
        // Approximate size (header + payload)
        64 + self.payload.len()
    }
}

/// Acknowledgment range.
#[derive(Debug, Clone)]
pub struct AckRange {
    /// First packet number in range
    pub first: u64,
    /// Last packet number in range
    pub last: u64,
}

impl AckRange {
    /// Create a new acknowledgment range.
    pub fn new(first: u64, last: u64) -> Self {
        Self { first, last }
    }
    
    /// Check if a packet number is in this range.
    pub fn contains(&self, packet_number: u64) -> bool {
        packet_number >= self.first && packet_number <= self.last
    }
    
    /// Get the size of this range.
    pub fn size(&self) -> u64 {
        self.last - self.first + 1
    }
}

/// Acknowledgment frame.
#[derive(Debug, Clone)]
pub struct AckFrame {
    /// Largest acknowledged packet number
    pub largest_acked: u64,
    /// ACK delay
    pub ack_delay: Duration,
    /// Acknowledgment ranges
    pub ranges: Vec<AckRange>,
}

impl AckFrame {
    /// Create a new ACK frame.
    pub fn new(largest_acked: u64, ack_delay: Duration) -> Self {
        Self {
            largest_acked,
            ack_delay,
            ranges: vec![AckRange::new(largest_acked, largest_acked)],
        }
    }
    
    /// Add an acknowledgment range.
    pub fn add_range(&mut self, first: u64, last: u64) {
        self.ranges.push(AckRange::new(first, last));
        // Sort ranges by first packet number
        self.ranges.sort_by_key(|r| r.first);
    }
    
    /// Check if a packet number is acknowledged.
    pub fn is_acked(&self, packet_number: u64) -> bool {
        self.ranges.iter().any(|range| range.contains(packet_number))
    }
    
    /// Encode the ACK frame to bytes.
    pub fn encode(&self) -> H3Result<Bytes> {
        let mut buffer = BytesMut::with_capacity(256);
        
        // Frame type (ACK = 0x02)
        buffer.put_u8(0x02);
        
        // Largest acknowledged
        buffer.put_u64(self.largest_acked);
        
        // ACK delay (in microseconds)
        buffer.put_u64(self.ack_delay.as_micros() as u64);
        
        // Number of ACK ranges
        buffer.put_u64(self.ranges.len() as u64);
        
        // ACK ranges
        for range in &self.ranges {
            buffer.put_u64(range.first);
            buffer.put_u64(range.last);
        }
        
        Ok(buffer.freeze())
    }
    
    /// Decode an ACK frame from bytes.
    pub fn decode(mut data: &[u8]) -> H3Result<Self> {
        if data.remaining() < 1 {
            return Err(TransportError::InvalidFrame("Empty ACK frame".to_string()).into());
        }
        
        let frame_type = data.get_u8();
        if frame_type != 0x02 {
            return Err(TransportError::InvalidFrame("Not an ACK frame".to_string()).into());
        }
        
        if data.remaining() < 24 {
            return Err(TransportError::InvalidFrame("Incomplete ACK frame".to_string()).into());
        }
        
        let largest_acked = data.get_u64();
        let ack_delay_micros = data.get_u64();
        let ack_delay = Duration::from_micros(ack_delay_micros);
        let range_count = data.get_u64() as usize;
        
        let mut ranges = Vec::with_capacity(range_count);
        for _ in 0..range_count {
            if data.remaining() < 16 {
                return Err(TransportError::InvalidFrame("Incomplete ACK range".to_string()).into());
            }
            let first = data.get_u64();
            let last = data.get_u64();
            ranges.push(AckRange::new(first, last));
        }
        
        Ok(Self {
            largest_acked,
            ack_delay,
            ranges,
        })
    }
}

/// Packet acknowledgment tracker.
#[derive(Debug)]
pub struct AckTracker {
    /// Packets waiting for acknowledgment
    unacked_packets: HashMap<u64, Packet>,
    /// Received packet numbers
    received_packets: VecDeque<u64>,
    /// Last ACK frame sent
    last_ack_sent: Option<Instant>,
    /// ACK delay threshold
    ack_delay_threshold: Duration,
}

impl AckTracker {
    /// Create a new acknowledgment tracker.
    pub fn new() -> Self {
        Self {
            unacked_packets: HashMap::new(),
            received_packets: VecDeque::new(),
            last_ack_sent: None,
            ack_delay_threshold: Duration::from_millis(25),
        }
    }
    
    /// Track a sent packet.
    pub fn track_sent(&mut self, packet: Packet) {
        if packet.ack_eliciting {
            self.unacked_packets.insert(packet.header.packet_number, packet);
        }
    }
    
    /// Track a received packet.
    pub fn track_received(&mut self, packet_number: u64) {
        self.received_packets.push_back(packet_number);
        
        // Keep only recent packet numbers
        while self.received_packets.len() > 1000 {
            self.received_packets.pop_front();
        }
    }
    
    /// Process an ACK frame.
    pub fn process_ack(&mut self, ack_frame: &AckFrame) -> Vec<Packet> {
        let mut acked_packets = Vec::new();
        
        // Remove acknowledged packets
        self.unacked_packets.retain(|&packet_number, packet| {
            if ack_frame.is_acked(packet_number) {
                acked_packets.push(packet.clone());
                false
            } else {
                true
            }
        });
        
        acked_packets
    }
    
    /// Generate an ACK frame for received packets.
    pub fn generate_ack(&mut self) -> Option<AckFrame> {
        if self.received_packets.is_empty() {
            return None;
        }
        
        // Check if we should send an ACK
        let should_ack = self.last_ack_sent
            .map(|last| last.elapsed() > self.ack_delay_threshold)
            .unwrap_or(true);
        
        if !should_ack {
            return None;
        }
        
        // Find the largest received packet number
        let largest_acked = *self.received_packets.iter().max()?;
        
        // Calculate ACK delay
        let ack_delay = self.last_ack_sent
            .map(|last| last.elapsed())
            .unwrap_or(Duration::ZERO);
        
        let mut ack_frame = AckFrame::new(largest_acked, ack_delay);
        
        // Add ranges for received packets
        let mut sorted_packets: Vec<u64> = self.received_packets.iter().cloned().collect();
        sorted_packets.sort_unstable();
        
        let mut range_start = sorted_packets[0];
        let mut range_end = sorted_packets[0];
        
        for &packet_num in sorted_packets.iter().skip(1) {
            if packet_num == range_end + 1 {
                range_end = packet_num;
            } else {
                if range_start != range_end {
                    ack_frame.add_range(range_start, range_end);
                }
                range_start = packet_num;
                range_end = packet_num;
            }
        }
        
        // Add the final range
        if range_start != range_end {
            ack_frame.add_range(range_start, range_end);
        }
        
        self.last_ack_sent = Some(Instant::now());
        Some(ack_frame)
    }
    
    /// Get packets that need retransmission.
    pub fn get_lost_packets(&self, rto: Duration) -> Vec<Packet> {
        let now = Instant::now();
        self.unacked_packets
            .values()
            .filter(|packet| now.duration_since(packet.timestamp) > rto)
            .cloned()
            .collect()
    }
    
    /// Get the number of unacknowledged packets.
    pub fn unacked_count(&self) -> usize {
        self.unacked_packets.len()
    }
}

impl Default for AckTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Loss detection and recovery.
#[derive(Debug)]
pub struct LossDetector {
    /// Round-trip time estimator
    rtt_estimator: RttEstimator,
    /// Retransmission timeout
    rto: Duration,
    /// Loss detection timer
    loss_timer: Option<Instant>,
    /// Packet threshold for fast retransmit
    packet_threshold: u64,
    /// Time threshold for loss detection
    time_threshold: Duration,
}

impl LossDetector {
    /// Create a new loss detector.
    pub fn new() -> Self {
        Self {
            rtt_estimator: RttEstimator::new(),
            rto: Duration::from_millis(500), // Initial RTO
            loss_timer: None,
            packet_threshold: 3,
            time_threshold: Duration::from_millis(9), // 9/8 * max_ack_delay
        }
    }
    
    /// Update RTT estimate.
    pub fn update_rtt(&mut self, rtt_sample: Duration) {
        self.rtt_estimator.update(rtt_sample);
        self.rto = self.rtt_estimator.rto();
    }
    
    /// Detect lost packets.
    pub fn detect_loss(&mut self, ack_tracker: &AckTracker) -> Vec<u64> {
        let mut lost_packets = Vec::new();
        let now = Instant::now();
        
        for (&packet_number, packet) in &ack_tracker.unacked_packets {
            // Time-based loss detection
            if now.duration_since(packet.timestamp) > self.rto {
                lost_packets.push(packet_number);
            }
        }
        
        lost_packets
    }
    
    /// Get the current RTO.
    pub fn rto(&self) -> Duration {
        self.rto
    }
}

impl Default for LossDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// RTT estimator.
#[derive(Debug)]
pub struct RttEstimator {
    /// Smoothed RTT
    srtt: Option<Duration>,
    /// RTT variation
    rttvar: Duration,
    /// Minimum RTT observed
    min_rtt: Option<Duration>,
}

impl RttEstimator {
    /// Create a new RTT estimator.
    pub fn new() -> Self {
        Self {
            srtt: None,
            rttvar: Duration::from_millis(250), // Initial RTT variance
            min_rtt: None,
        }
    }
    
    /// Update RTT estimate with a new sample.
    pub fn update(&mut self, rtt_sample: Duration) {
        // Update minimum RTT
        self.min_rtt = Some(
            self.min_rtt
                .map(|min| min.min(rtt_sample))
                .unwrap_or(rtt_sample)
        );
        
        match self.srtt {
            None => {
                // First RTT sample
                self.srtt = Some(rtt_sample);
                self.rttvar = rtt_sample / 2;
            }
            Some(srtt) => {
                // Update smoothed RTT and RTT variance
                let rttvar_sample = if rtt_sample > srtt {
                    rtt_sample - srtt
                } else {
                    srtt - rtt_sample
                };
                
                self.rttvar = (3 * self.rttvar + rttvar_sample) / 4;
                self.srtt = Some((7 * srtt + rtt_sample) / 8);
            }
        }
    }
    
    /// Get the current RTO.
    pub fn rto(&self) -> Duration {
        let base_rto = self.srtt.unwrap_or(Duration::from_millis(500)) + 4 * self.rttvar;
        base_rto.max(Duration::from_millis(200)) // Minimum RTO of 200ms
    }
    
    /// Get the smoothed RTT.
    pub fn srtt(&self) -> Option<Duration> {
        self.srtt
    }
    
    /// Get the minimum RTT.
    pub fn min_rtt(&self) -> Option<Duration> {
        self.min_rtt
    }
}

impl Default for RttEstimator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_packet_type_parsing() {
        assert_eq!(PacketType::from_byte(0x80), Some(PacketType::Initial));
        assert_eq!(PacketType::from_byte(0x90), Some(PacketType::ZeroRTT));
        assert_eq!(PacketType::from_byte(0xA0), Some(PacketType::Handshake));
        assert_eq!(PacketType::from_byte(0x40), Some(PacketType::OneRTT));
        assert_eq!(PacketType::from_byte(0xFF), None);
    }
    
    #[test]
    fn test_packet_header_encoding() {
        let connection_id = ConnectionId([1; 16]);
        let header = PacketHeader::new(PacketType::Initial, connection_id, 42);
        
        let encoded = header.encode().unwrap();
        assert!(!encoded.is_empty());
        
        let (decoded, _) = PacketHeader::decode(&encoded).unwrap();
        assert_eq!(decoded.packet_type, PacketType::Initial);
        assert_eq!(decoded.connection_id, connection_id);
        assert_eq!(decoded.packet_number, 42);
    }
    
    #[test]
    fn test_packet_encoding() {
        let connection_id = ConnectionId([2; 16]);
        let payload = Bytes::from("test payload");
        let packet = Packet::new(PacketType::OneRTT, connection_id, 123, payload.clone());
        
        let encoded = packet.encode().unwrap();
        let decoded = Packet::decode(&encoded).unwrap();
        
        assert_eq!(decoded.header.packet_type, PacketType::OneRTT);
        assert_eq!(decoded.header.connection_id, connection_id);
        assert_eq!(decoded.header.packet_number, 123);
        assert_eq!(decoded.payload, payload);
    }
    
    #[test]
    fn test_ack_frame() {
        let mut ack_frame = AckFrame::new(100, Duration::from_millis(10));
        ack_frame.add_range(50, 60);
        ack_frame.add_range(80, 90);
        
        assert!(ack_frame.is_acked(55));
        assert!(ack_frame.is_acked(85));
        assert!(ack_frame.is_acked(100));
        assert!(!ack_frame.is_acked(70));
        
        let encoded = ack_frame.encode().unwrap();
        let decoded = AckFrame::decode(&encoded).unwrap();
        
        assert_eq!(decoded.largest_acked, 100);
        assert_eq!(decoded.ranges.len(), 3);
    }
    
    #[test]
    fn test_ack_tracker() {
        let mut tracker = AckTracker::new();
        
        // Track some received packets
        tracker.track_received(1);
        tracker.track_received(2);
        tracker.track_received(4);
        tracker.track_received(5);
        
        let ack_frame = tracker.generate_ack().unwrap();
        assert_eq!(ack_frame.largest_acked, 5);
        assert!(ack_frame.is_acked(1));
        assert!(ack_frame.is_acked(2));
        assert!(!ack_frame.is_acked(3));
        assert!(ack_frame.is_acked(4));
        assert!(ack_frame.is_acked(5));
    }
    
    #[test]
    fn test_rtt_estimator() {
        let mut estimator = RttEstimator::new();
        
        // First sample
        estimator.update(Duration::from_millis(100));
        assert_eq!(estimator.srtt(), Some(Duration::from_millis(100)));
        
        // Second sample
        estimator.update(Duration::from_millis(200));
        let srtt = estimator.srtt().unwrap();
        assert!(srtt > Duration::from_millis(100));
        assert!(srtt < Duration::from_millis(200));
        
        // RTO should be reasonable
        let rto = estimator.rto();
        assert!(rto >= Duration::from_millis(200));
        assert!(rto <= Duration::from_secs(2));
    }
}

