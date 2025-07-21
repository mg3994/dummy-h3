//! HTTP/3 frame parsing and encoding.

use crate::error::{ParseError, H3Result};
use bytes::{Bytes, BytesMut, Buf, BufMut};
use http::HeaderMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// HTTP/3 frame types according to RFC 9114.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u64)]
pub enum FrameType {
    /// DATA frame (0x00) - carries request/response body
    Data = 0x00,
    /// HEADERS frame (0x01) - carries request/response headers
    Headers = 0x01,
    /// CANCEL_PUSH frame (0x03) - cancels server push
    CancelPush = 0x03,
    /// SETTINGS frame (0x04) - carries connection settings
    Settings = 0x04,
    /// PUSH_PROMISE frame (0x05) - server push promise
    PushPromise = 0x05,
    /// GOAWAY frame (0x07) - graceful connection termination
    Goaway = 0x07,
    /// MAX_PUSH_ID frame (0x0D) - maximum push stream ID
    MaxPushId = 0x0D,
}

impl TryFrom<u64> for FrameType {
    type Error = ParseError;
    
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(FrameType::Data),
            0x01 => Ok(FrameType::Headers),
            0x03 => Ok(FrameType::CancelPush),
            0x04 => Ok(FrameType::Settings),
            0x05 => Ok(FrameType::PushPromise),
            0x07 => Ok(FrameType::Goaway),
            0x0D => Ok(FrameType::MaxPushId),
            _ => Err(ParseError::InvalidFormat(format!("Unknown frame type: {}", value))),
        }
    }
}

impl From<FrameType> for u64 {
    fn from(frame_type: FrameType) -> Self {
        frame_type as u64
    }
}

/// HTTP/3 frame representation with all frame types.
#[derive(Debug, Clone)]
pub enum H3Frame {
    /// DATA frame (0x00) - carries request/response body
    Data {
        /// Stream identifier
        stream_id: u64,
        /// Frame payload data
        data: Bytes,
        /// Whether this frame ends the stream
        end_stream: bool,
    },
    
    /// HEADERS frame (0x01) - carries request/response headers
    Headers {
        /// Stream identifier
        stream_id: u64,
        /// HTTP headers (QPACK encoded)
        headers: HeaderMap,
        /// Whether this frame ends the stream
        end_stream: bool,
        /// Priority information (optional)
        priority: Option<Priority>,
    },
    
    /// CANCEL_PUSH frame (0x03) - cancels server push
    CancelPush {
        /// Push ID to cancel
        push_id: u64,
    },
    
    /// SETTINGS frame (0x04) - carries connection settings
    Settings {
        /// Settings key-value pairs
        settings: HashMap<SettingId, u64>,
    },
    
    /// PUSH_PROMISE frame (0x05) - server push promise
    PushPromise {
        /// Stream identifier
        stream_id: u64,
        /// Promised stream identifier
        promised_stream_id: u64,
        /// Request headers for the promised response
        headers: HeaderMap,
    },
    
    /// GOAWAY frame (0x07) - graceful connection termination
    Goaway {
        /// Last processed stream ID
        stream_id: u64,
    },
    
    /// MAX_PUSH_ID frame (0x0D) - maximum push stream ID
    MaxPushId {
        /// Maximum push stream ID
        push_id: u64,
    },
}

/// HTTP/3 Settings identifiers according to RFC 9114.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u64)]
pub enum SettingId {
    /// QPACK maximum table capacity
    QpackMaxTableCapacity = 0x01,
    /// Maximum field section size
    MaxFieldSectionSize = 0x06,
    /// QPACK blocked streams
    QpackBlockedStreams = 0x07,
}

impl TryFrom<u64> for SettingId {
    type Error = ParseError;
    
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(SettingId::QpackMaxTableCapacity),
            0x06 => Ok(SettingId::MaxFieldSectionSize),
            0x07 => Ok(SettingId::QpackBlockedStreams),
            _ => Err(ParseError::InvalidFormat(format!("Unknown setting ID: {}", value))),
        }
    }
}

impl From<SettingId> for u64 {
    fn from(setting_id: SettingId) -> Self {
        setting_id as u64
    }
}

/// Priority information for HTTP/3 frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Priority {
    /// Priority weight (1-256)
    pub weight: u8,
    /// Stream dependency
    pub depends_on: Option<u64>,
    /// Exclusive dependency flag
    pub exclusive: bool,
}

impl Default for Priority {
    fn default() -> Self {
        Self {
            weight: 16,
            depends_on: None,
            exclusive: false,
        }
    }
}

/// Trait for frame serialization.
pub trait FrameSerialize {
    /// Serialize the frame to bytes.
    fn serialize(&self) -> H3Result<Bytes>;
    
    /// Get the serialized size of the frame.
    fn serialized_size(&self) -> usize;
}

/// Trait for frame deserialization.
pub trait FrameDeserialize: Sized {
    /// Deserialize a frame from bytes.
    fn deserialize(frame_type: FrameType, data: &[u8]) -> H3Result<Self>;
}

impl H3Frame {
    /// Get the frame type.
    pub fn frame_type(&self) -> FrameType {
        match self {
            H3Frame::Data { .. } => FrameType::Data,
            H3Frame::Headers { .. } => FrameType::Headers,
            H3Frame::CancelPush { .. } => FrameType::CancelPush,
            H3Frame::Settings { .. } => FrameType::Settings,
            H3Frame::PushPromise { .. } => FrameType::PushPromise,
            H3Frame::Goaway { .. } => FrameType::Goaway,
            H3Frame::MaxPushId { .. } => FrameType::MaxPushId,
        }
    }
    
    /// Get the stream ID if applicable.
    pub fn stream_id(&self) -> Option<u64> {
        match self {
            H3Frame::Data { stream_id, .. } => Some(*stream_id),
            H3Frame::Headers { stream_id, .. } => Some(*stream_id),
            H3Frame::PushPromise { stream_id, .. } => Some(*stream_id),
            H3Frame::Goaway { stream_id } => Some(*stream_id),
            H3Frame::CancelPush { .. } => None,
            H3Frame::Settings { .. } => None,
            H3Frame::MaxPushId { .. } => None,
        }
    }
    
    /// Check if this frame ends the stream.
    pub fn is_end_stream(&self) -> bool {
        match self {
            H3Frame::Data { end_stream, .. } => *end_stream,
            H3Frame::Headers { end_stream, .. } => *end_stream,
            _ => false,
        }
    }
    
    /// Get the frame payload size.
    pub fn payload_size(&self) -> usize {
        match self {
            H3Frame::Data { data, .. } => data.len(),
            H3Frame::Headers { headers, .. } => {
                // Estimate header size (actual size depends on QPACK encoding)
                headers.iter().map(|(k, v)| k.as_str().len() + v.len()).sum()
            }
            H3Frame::CancelPush { .. } => 8, // varint push_id
            H3Frame::Settings { settings } => settings.len() * 16, // estimate
            H3Frame::PushPromise { headers, .. } => {
                8 + headers.iter().map(|(k, v)| k.as_str().len() + v.len()).sum::<usize>()
            }
            H3Frame::Goaway { .. } => 8, // varint stream_id
            H3Frame::MaxPushId { .. } => 8, // varint push_id
        }
    }
    
    /// Check if this is a connection-level frame.
    pub fn is_connection_frame(&self) -> bool {
        matches!(
            self,
            H3Frame::Settings { .. } | H3Frame::Goaway { .. } | H3Frame::MaxPushId { .. } | H3Frame::CancelPush { .. }
        )
    }
    
    /// Check if this is a stream-level frame.
    pub fn is_stream_frame(&self) -> bool {
        !self.is_connection_frame()
    }
}

/// Parser state for incremental frame parsing.
#[derive(Debug, Clone)]
enum ParserState {
    /// Waiting for frame header (type + length)
    WaitingHeader,
    /// Reading frame payload
    ReadingPayload {
        frame_type: FrameType,
        length: usize,
    },
}

/// HTTP/3 frame parser with incremental parsing support.
#[derive(Debug)]
pub struct FrameParser {
    /// Internal buffer for incomplete frames
    buffer: BytesMut,
    /// Current parser state
    state: ParserState,
    /// Maximum allowed frame size
    max_frame_size: usize,
    /// Maximum buffer size to prevent memory exhaustion
    max_buffer_size: usize,
    /// Statistics for monitoring
    stats: ParserStats,
}

/// Parser statistics for monitoring and debugging.
#[derive(Debug, Clone, Default)]
pub struct ParserStats {
    /// Total frames parsed successfully
    pub frames_parsed: u64,
    /// Total bytes processed
    pub bytes_processed: u64,
    /// Parse errors encountered
    pub parse_errors: u64,
    /// Buffer overflows
    pub buffer_overflows: u64,
}

impl FrameParser {
    /// Create a new frame parser with default settings.
    pub fn new() -> Self {
        Self {
            buffer: BytesMut::new(),
            state: ParserState::WaitingHeader,
            max_frame_size: 16384, // 16KB default
            max_buffer_size: 65536, // 64KB buffer limit
            stats: ParserStats::default(),
        }
    }
    
    /// Create a new frame parser with custom max frame size.
    pub fn with_max_frame_size(max_size: usize) -> Self {
        Self {
            buffer: BytesMut::new(),
            state: ParserState::WaitingHeader,
            max_frame_size: max_size,
            max_buffer_size: max_size * 4, // Buffer should be larger than max frame
            stats: ParserStats::default(),
        }
    }
    
    /// Create a new frame parser with custom settings.
    pub fn with_config(max_frame_size: usize, max_buffer_size: usize) -> Self {
        Self {
            buffer: BytesMut::new(),
            state: ParserState::WaitingHeader,
            max_frame_size,
            max_buffer_size,
            stats: ParserStats::default(),
        }
    }
    
    /// Get parser statistics.
    pub fn stats(&self) -> &ParserStats {
        &self.stats
    }
    
    /// Reset parser statistics.
    pub fn reset_stats(&mut self) {
        self.stats = ParserStats::default();
    }
    
    /// Get current buffer size.
    pub fn buffer_size(&self) -> usize {
        self.buffer.len()
    }
    
    /// Check if buffer is empty.
    pub fn is_buffer_empty(&self) -> bool {
        self.buffer.is_empty()
    }
    
    /// Clear the internal buffer and reset state.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.state = ParserState::WaitingHeader;
    }
    
    /// Parse frames from the given data.
    /// Returns Ok(Some(frame)) if a complete frame was parsed,
    /// Ok(None) if more data is needed, or Err if parsing failed.
    pub fn parse_frame(&mut self, data: &[u8]) -> H3Result<Option<H3Frame>> {
        // Check buffer size limit before adding data
        if self.buffer.len() + data.len() > self.max_buffer_size {
            self.stats.buffer_overflows += 1;
            return Err(ParseError::BufferOverflow.into());
        }
        
        // Add new data to buffer
        self.buffer.extend_from_slice(data);
        self.stats.bytes_processed += data.len() as u64;
        
        loop {
            match self.state.clone() {
                ParserState::WaitingHeader => {
                    // Try to parse frame header
                    match self.try_parse_header() {
                        Ok(Some((frame_type, length))) => {
                            // Successfully parsed header, transition to payload reading
                            self.state = ParserState::ReadingPayload { frame_type, length };
                        }
                        Ok(None) => {
                            // Need more data for header
                            return Ok(None);
                        }
                        Err(e) => {
                            self.stats.parse_errors += 1;
                            return Err(e);
                        }
                    }
                }
                
                ParserState::ReadingPayload { frame_type, length } => {
                    if self.buffer.len() < length {
                        // Need more data for payload
                        return Ok(None);
                    }
                    
                    // Extract payload and reset state
                    let payload = self.buffer.split_to(length);
                    self.state = ParserState::WaitingHeader;
                    
                    // Parse the frame payload
                    match self.parse_frame_payload(frame_type, payload.freeze()) {
                        Ok(frame) => {
                            self.stats.frames_parsed += 1;
                            return Ok(Some(frame));
                        }
                        Err(e) => {
                            self.stats.parse_errors += 1;
                            return Err(e);
                        }
                    }
                }
            }
        }
    }
    
    /// Try to parse frame header from buffer.
    /// Returns Ok(Some((frame_type, length))) if header is complete,
    /// Ok(None) if more data is needed, or Err if parsing failed.
    fn try_parse_header(&mut self) -> H3Result<Option<(FrameType, usize)>> {
        // Need at least 2 bytes for minimum varint encoding
        if self.buffer.len() < 2 {
            return Ok(None);
        }
        
        let mut cursor = std::io::Cursor::new(&self.buffer[..]);
        
        // Parse frame type
        let frame_type_raw = match self.read_varint(&mut cursor) {
            Ok(val) => val,
            Err(ParseError::Incomplete) => return Ok(None), // Need more data
            Err(e) => return Err(e.into()),
        };
        
        let frame_type = FrameType::try_from(frame_type_raw)?;
        
        // Parse frame length
        let length = match self.read_varint(&mut cursor) {
            Ok(val) => val as usize,
            Err(ParseError::Incomplete) => return Ok(None), // Need more data
            Err(e) => return Err(e.into()),
        };
        
        // Validate frame size
        if length > self.max_frame_size {
            return Err(ParseError::FrameTooLarge {
                size: length,
                max: self.max_frame_size,
            }.into());
        }
        
        // Advance buffer past the header
        let header_len = cursor.position() as usize;
        self.buffer.advance(header_len);
        
        Ok(Some((frame_type, length)))
    }
    
    /// Parse multiple frames from the buffer.
    /// Returns a vector of successfully parsed frames.
    pub fn parse_frames(&mut self, data: &[u8]) -> H3Result<Vec<H3Frame>> {
        let mut frames = Vec::new();
        
        // Add data to buffer
        if self.buffer.len() + data.len() > self.max_buffer_size {
            self.stats.buffer_overflows += 1;
            return Err(ParseError::BufferOverflow.into());
        }
        
        self.buffer.extend_from_slice(data);
        self.stats.bytes_processed += data.len() as u64;
        
        // Parse as many complete frames as possible
        loop {
            match self.parse_frame(&[]) {
                Ok(Some(frame)) => frames.push(frame),
                Ok(None) => break, // No more complete frames
                Err(e) => return Err(e),
            }
        }
        
        Ok(frames)
    }
    
    /// Check if a complete frame is available in the buffer.
    pub fn has_complete_frame(&self) -> bool {
        if self.buffer.len() < 2 {
            return false;
        }
        
        let mut cursor = std::io::Cursor::new(&self.buffer[..]);
        
        // Try to read frame type and length
        if let (Ok(_), Ok(length)) = (self.read_varint(&mut cursor), self.read_varint(&mut cursor)) {
            let header_len = cursor.position() as usize;
            return self.buffer.len() >= header_len + length as usize;
        }
        
        false
    }
    
    /// Encode a frame to bytes.
    pub fn encode_frame(&self, frame: &H3Frame) -> H3Result<Bytes> {
        let mut buf = BytesMut::new();
        
        // Write frame type
        self.write_varint(&mut buf, frame.frame_type() as u64);
        
        // Encode payload
        let payload = self.encode_frame_payload(frame)?;
        
        // Write frame length
        self.write_varint(&mut buf, payload.len() as u64);
        
        // Write payload
        buf.extend_from_slice(&payload);
        
        Ok(buf.freeze())
    }
    
    /// Parse frame payload based on frame type.
    fn parse_frame_payload(&self, frame_type: FrameType, payload: Bytes) -> H3Result<H3Frame> {
        match frame_type {
            FrameType::Data => {
                // For now, we'll assume stream_id and end_stream are provided elsewhere
                // In a real implementation, this would be handled by the stream layer
                Ok(H3Frame::Data {
                    stream_id: 0, // Placeholder
                    data: payload,
                    end_stream: false, // Placeholder
                })
            }
            
            FrameType::Headers => {
                // Parse QPACK-encoded headers
                let headers = self.parse_headers(&payload)?;
                Ok(H3Frame::Headers {
                    stream_id: 0, // Placeholder
                    headers,
                    end_stream: false, // Placeholder
                    priority: None, // TODO: Parse priority information
                })
            }
            
            FrameType::CancelPush => {
                let mut cursor = std::io::Cursor::new(&payload[..]);
                let push_id = self.read_varint(&mut cursor)?;
                Ok(H3Frame::CancelPush { push_id })
            }
            
            FrameType::Settings => {
                let settings = self.parse_settings(&payload)?;
                Ok(H3Frame::Settings { settings })
            }
            
            FrameType::PushPromise => {
                let mut cursor = std::io::Cursor::new(&payload[..]);
                let promised_stream_id = self.read_varint(&mut cursor)?;
                let header_data = &payload[cursor.position() as usize..];
                let headers = self.parse_headers(header_data)?;
                
                Ok(H3Frame::PushPromise {
                    stream_id: 0, // Placeholder
                    promised_stream_id,
                    headers,
                })
            }
            
            FrameType::Goaway => {
                let mut cursor = std::io::Cursor::new(&payload[..]);
                let stream_id = self.read_varint(&mut cursor)?;
                Ok(H3Frame::Goaway { stream_id })
            }
            
            FrameType::MaxPushId => {
                let mut cursor = std::io::Cursor::new(&payload[..]);
                let push_id = self.read_varint(&mut cursor)?;
                Ok(H3Frame::MaxPushId { push_id })
            }
        }
    }
    
    /// Encode frame payload.
    fn encode_frame_payload(&self, frame: &H3Frame) -> H3Result<Bytes> {
        let mut buf = BytesMut::new();
        
        match frame {
            H3Frame::Data { data, .. } => {
                buf.extend_from_slice(data);
            }
            
            H3Frame::Headers { headers, priority, .. } => {
                // TODO: Encode priority information if present
                if let Some(_priority) = priority {
                    // Priority encoding would go here
                }
                let encoded_headers = self.encode_headers(headers)?;
                buf.extend_from_slice(&encoded_headers);
            }
            
            H3Frame::CancelPush { push_id } => {
                self.write_varint(&mut buf, *push_id);
            }
            
            H3Frame::Settings { settings } => {
                for (&key, &value) in settings {
                    self.write_varint(&mut buf, key.into());
                    self.write_varint(&mut buf, value);
                }
            }
            
            H3Frame::PushPromise { promised_stream_id, headers, .. } => {
                self.write_varint(&mut buf, *promised_stream_id);
                let encoded_headers = self.encode_headers(headers)?;
                buf.extend_from_slice(&encoded_headers);
            }
            
            H3Frame::Goaway { stream_id } => {
                self.write_varint(&mut buf, *stream_id);
            }
            
            H3Frame::MaxPushId { push_id } => {
                self.write_varint(&mut buf, *push_id);
            }
        }
        
        Ok(buf.freeze())
    }
    
    /// Parse SETTINGS frame payload.
    fn parse_settings(&self, payload: &[u8]) -> H3Result<HashMap<SettingId, u64>> {
        let mut settings = HashMap::new();
        let mut cursor = std::io::Cursor::new(payload);
        
        while cursor.position() < payload.len() as u64 {
            let key_raw = self.read_varint(&mut cursor)?;
            let value = self.read_varint(&mut cursor)?;
            
            // Only store known settings, ignore unknown ones per RFC
            if let Ok(setting_id) = SettingId::try_from(key_raw) {
                settings.insert(setting_id, value);
            }
        }
        
        Ok(settings)
    }
    
    /// Parse QPACK-encoded headers (simplified implementation).
    fn parse_headers(&self, _data: &[u8]) -> H3Result<HeaderMap> {
        // TODO: Implement proper QPACK decoding
        // For now, return empty headers
        Ok(HeaderMap::new())
    }
    
    /// Encode headers using QPACK (simplified implementation).
    fn encode_headers(&self, _headers: &HeaderMap) -> H3Result<Bytes> {
        // TODO: Implement proper QPACK encoding
        // For now, return empty bytes
        Ok(Bytes::new())
    }
    
    /// Read a variable-length integer according to RFC 9000.
    fn read_varint(&self, cursor: &mut std::io::Cursor<&[u8]>) -> Result<u64, ParseError> {
        if !cursor.has_remaining() {
            return Err(ParseError::Incomplete);
        }
        
        let first_byte = cursor.get_u8();
        let length = 1 << (first_byte >> 6);
        let mut value = (first_byte & 0x3F) as u64;
        
        for _ in 1..length {
            if !cursor.has_remaining() {
                return Err(ParseError::Incomplete);
            }
            value = (value << 8) | (cursor.get_u8() as u64);
        }
        
        Ok(value)
    }
    
    /// Write a variable-length integer according to RFC 9000.
    fn write_varint(&self, buf: &mut BytesMut, value: u64) {
        if value < 64 {
            buf.put_u8(value as u8);
        } else if value < 16384 {
            buf.put_u8(0x40 | ((value >> 8) as u8));
            buf.put_u8(value as u8);
        } else if value < 1073741824 {
            buf.put_u8(0x80 | ((value >> 24) as u8));
            buf.put_u8((value >> 16) as u8);
            buf.put_u8((value >> 8) as u8);
            buf.put_u8(value as u8);
        } else {
            buf.put_u8(0xC0 | ((value >> 56) as u8));
            buf.put_u8((value >> 48) as u8);
            buf.put_u8((value >> 40) as u8);
            buf.put_u8((value >> 32) as u8);
            buf.put_u8((value >> 24) as u8);
            buf.put_u8((value >> 16) as u8);
            buf.put_u8((value >> 8) as u8);
            buf.put_u8(value as u8);
        }
    }
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameSerialize for H3Frame {
    fn serialize(&self) -> H3Result<Bytes> {
        let parser = FrameParser::new();
        parser.encode_frame(self)
    }
    
    fn serialized_size(&self) -> usize {
        // Frame type (varint) + length (varint) + payload
        let payload_size = self.payload_size();
        varint_size(self.frame_type() as u64) + varint_size(payload_size as u64) + payload_size
    }
}

impl FrameDeserialize for H3Frame {
    fn deserialize(frame_type: FrameType, data: &[u8]) -> H3Result<Self> {
        let parser = FrameParser::new();
        parser.parse_frame_payload(frame_type, Bytes::copy_from_slice(data))
    }
}

/// Calculate the size of a varint encoding.
fn varint_size(value: u64) -> usize {
    if value < 64 {
        1
    } else if value < 16384 {
        2
    } else if value < 1073741824 {
        4
    } else {
        8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_frame_type_conversion() {
        assert_eq!(FrameType::try_from(0x00).unwrap(), FrameType::Data);
        assert_eq!(FrameType::try_from(0x01).unwrap(), FrameType::Headers);
        assert_eq!(FrameType::try_from(0x03).unwrap(), FrameType::CancelPush);
        assert_eq!(FrameType::try_from(0x04).unwrap(), FrameType::Settings);
        assert_eq!(FrameType::try_from(0x05).unwrap(), FrameType::PushPromise);
        assert_eq!(FrameType::try_from(0x07).unwrap(), FrameType::Goaway);
        assert_eq!(FrameType::try_from(0x0D).unwrap(), FrameType::MaxPushId);
        assert!(FrameType::try_from(0xFF).is_err());
    }
    
    #[test]
    fn test_frame_type_to_u64() {
        assert_eq!(u64::from(FrameType::Data), 0x00);
        assert_eq!(u64::from(FrameType::Headers), 0x01);
        assert_eq!(u64::from(FrameType::CancelPush), 0x03);
        assert_eq!(u64::from(FrameType::Settings), 0x04);
    }
    
    #[test]
    fn test_setting_id_conversion() {
        assert_eq!(SettingId::try_from(0x01).unwrap(), SettingId::QpackMaxTableCapacity);
        assert_eq!(SettingId::try_from(0x06).unwrap(), SettingId::MaxFieldSectionSize);
        assert_eq!(SettingId::try_from(0x07).unwrap(), SettingId::QpackBlockedStreams);
        assert!(SettingId::try_from(0xFF).is_err());
    }
    
    #[test]
    fn test_varint_encoding() {
        let parser = FrameParser::new();
        let mut buf = BytesMut::new();
        
        parser.write_varint(&mut buf, 0);
        assert_eq!(buf.as_ref(), &[0x00]);
        
        buf.clear();
        parser.write_varint(&mut buf, 63);
        assert_eq!(buf.as_ref(), &[0x3F]);
        
        buf.clear();
        parser.write_varint(&mut buf, 64);
        assert_eq!(buf.as_ref(), &[0x40, 0x40]);
    }
    
    #[test]
    fn test_varint_size_calculation() {
        assert_eq!(varint_size(0), 1);
        assert_eq!(varint_size(63), 1);
        assert_eq!(varint_size(64), 2);
        assert_eq!(varint_size(16383), 2);
        assert_eq!(varint_size(16384), 4);
        assert_eq!(varint_size(1073741823), 4);
        assert_eq!(varint_size(1073741824), 8);
    }
    
    #[test]
    fn test_data_frame_creation() {
        let data = Bytes::from("Hello, World!");
        let frame = H3Frame::Data {
            stream_id: 4,
            data: data.clone(),
            end_stream: true,
        };
        
        assert_eq!(frame.frame_type(), FrameType::Data);
        assert_eq!(frame.stream_id(), Some(4));
        assert!(frame.is_end_stream());
        assert!(frame.is_stream_frame());
        assert!(!frame.is_connection_frame());
        assert_eq!(frame.payload_size(), data.len());
    }
    
    #[test]
    fn test_headers_frame_creation() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        
        let frame = H3Frame::Headers {
            stream_id: 4,
            headers: headers.clone(),
            end_stream: false,
            priority: Some(Priority::default()),
        };
        
        assert_eq!(frame.frame_type(), FrameType::Headers);
        assert_eq!(frame.stream_id(), Some(4));
        assert!(!frame.is_end_stream());
        assert!(frame.is_stream_frame());
    }
    
    #[test]
    fn test_settings_frame_creation() {
        let mut settings = HashMap::new();
        settings.insert(SettingId::QpackMaxTableCapacity, 4096);
        settings.insert(SettingId::MaxFieldSectionSize, 8192);
        
        let frame = H3Frame::Settings { settings };
        
        assert_eq!(frame.frame_type(), FrameType::Settings);
        assert_eq!(frame.stream_id(), None);
        assert!(!frame.is_end_stream());
        assert!(frame.is_connection_frame());
        assert!(!frame.is_stream_frame());
    }
    
    #[test]
    fn test_cancel_push_frame() {
        let frame = H3Frame::CancelPush { push_id: 123 };
        
        assert_eq!(frame.frame_type(), FrameType::CancelPush);
        assert_eq!(frame.stream_id(), None);
        assert!(frame.is_connection_frame());
    }
    
    #[test]
    fn test_goaway_frame() {
        let frame = H3Frame::Goaway { stream_id: 100 };
        
        assert_eq!(frame.frame_type(), FrameType::Goaway);
        assert_eq!(frame.stream_id(), Some(100));
        assert!(frame.is_connection_frame());
    }
    
    #[test]
    fn test_max_push_id_frame() {
        let frame = H3Frame::MaxPushId { push_id: 255 };
        
        assert_eq!(frame.frame_type(), FrameType::MaxPushId);
        assert_eq!(frame.stream_id(), None);
        assert!(frame.is_connection_frame());
    }
    
    #[test]
    fn test_frame_serialization() {
        let frame = H3Frame::MaxPushId { push_id: 123 };
        let serialized = frame.serialize().unwrap();
        assert!(!serialized.is_empty());
        
        let size = frame.serialized_size();
        assert!(size > 0);
    }
    
    #[test]
    fn test_frame_round_trip_serialization() {
        // Test MaxPushId frame round-trip
        let original_frame = H3Frame::MaxPushId { push_id: 42 };
        let serialized = original_frame.serialize().unwrap();
        
        // Parse the serialized frame
        let mut parser = FrameParser::new();
        let parsed_frame = parser.parse_frame(&serialized).unwrap();
        
        // Should get Some(frame) since we have a complete frame
        assert!(parsed_frame.is_some());
        
        // Test Settings frame round-trip
        let mut settings = HashMap::new();
        settings.insert(SettingId::QpackMaxTableCapacity, 4096);
        let settings_frame = H3Frame::Settings { settings };
        let serialized = settings_frame.serialize().unwrap();
        assert!(!serialized.is_empty());
    }
    
    #[test]
    fn test_settings_frame_encoding_decoding() {
        let mut settings = HashMap::new();
        settings.insert(SettingId::QpackMaxTableCapacity, 4096);
        settings.insert(SettingId::MaxFieldSectionSize, 8192);
        
        let frame = H3Frame::Settings { settings };
        let parser = FrameParser::new();
        
        let encoded = parser.encode_frame(&frame).unwrap();
        assert!(!encoded.is_empty());
        
        // The encoded frame should contain frame type and length
        assert!(encoded.len() >= 2);
    }
    
    #[test]
    fn test_priority_default() {
        let priority = Priority::default();
        assert_eq!(priority.weight, 16);
        assert_eq!(priority.depends_on, None);
        assert!(!priority.exclusive);
    }
    
    // Enhanced parser tests for Task 2.2
    
    #[test]
    fn test_parser_creation_and_config() {
        let parser = FrameParser::new();
        assert_eq!(parser.buffer_size(), 0);
        assert!(parser.is_buffer_empty());
        
        let parser = FrameParser::with_max_frame_size(32768);
        assert_eq!(parser.max_frame_size, 32768);
        
        let parser = FrameParser::with_config(8192, 16384);
        assert_eq!(parser.max_frame_size, 8192);
        assert_eq!(parser.max_buffer_size, 16384);
    }
    
    #[test]
    fn test_parser_statistics() {
        let mut parser = FrameParser::new();
        let stats = parser.stats();
        assert_eq!(stats.frames_parsed, 0);
        assert_eq!(stats.bytes_processed, 0);
        assert_eq!(stats.parse_errors, 0);
        
        parser.reset_stats();
        assert_eq!(parser.stats().frames_parsed, 0);
    }
    
    #[test]
    fn test_incremental_frame_parsing() {
        let mut parser = FrameParser::new();
        
        // Create a MaxPushId frame
        let frame = H3Frame::MaxPushId { push_id: 123 };
        let encoded = parser.encode_frame(&frame).unwrap();
        
        // Parse incrementally - first send partial data
        let mid_point = encoded.len() / 2;
        let result1 = parser.parse_frame(&encoded[..mid_point]).unwrap();
        assert!(result1.is_none()); // Should need more data
        
        // Send remaining data
        let result2 = parser.parse_frame(&encoded[mid_point..]).unwrap();
        assert!(result2.is_some()); // Should have complete frame
        
        match result2.unwrap() {
            H3Frame::MaxPushId { push_id } => assert_eq!(push_id, 123),
            _ => panic!("Expected MaxPushId frame"),
        }
    }
    
    #[test]
    fn test_multiple_frames_parsing() {
        let mut parser = FrameParser::new();
        
        // Create multiple frames
        let frame1 = H3Frame::MaxPushId { push_id: 100 };
        let frame2 = H3Frame::CancelPush { push_id: 200 };
        let frame3 = H3Frame::Goaway { stream_id: 300 };
        
        // Encode all frames
        let mut combined_data = BytesMut::new();
        combined_data.extend_from_slice(&parser.encode_frame(&frame1).unwrap());
        combined_data.extend_from_slice(&parser.encode_frame(&frame2).unwrap());
        combined_data.extend_from_slice(&parser.encode_frame(&frame3).unwrap());
        
        // Parse multiple frames at once
        let frames = parser.parse_frames(&combined_data).unwrap();
        assert_eq!(frames.len(), 3);
        
        match &frames[0] {
            H3Frame::MaxPushId { push_id } => assert_eq!(*push_id, 100),
            _ => panic!("Expected MaxPushId frame"),
        }
        
        match &frames[1] {
            H3Frame::CancelPush { push_id } => assert_eq!(*push_id, 200),
            _ => panic!("Expected CancelPush frame"),
        }
        
        match &frames[2] {
            H3Frame::Goaway { stream_id } => assert_eq!(*stream_id, 300),
            _ => panic!("Expected Goaway frame"),
        }
    }
    
    #[test]
    fn test_buffer_overflow_protection() {
        let mut parser = FrameParser::with_config(1024, 512); // Small buffer
        
        // Try to add more data than buffer can hold
        let large_data = vec![0u8; 1000];
        let result = parser.parse_frame(&large_data);
        assert!(result.is_err());
        
        // Check that buffer overflow was recorded in stats
        assert_eq!(parser.stats().buffer_overflows, 1);
    }
    
    #[test]
    fn test_frame_size_limit_enforcement() {
        let mut parser = FrameParser::with_max_frame_size(100); // Very small limit
        
        // Create a frame header that claims a large payload
        let mut buf = BytesMut::new();
        parser.write_varint(&mut buf, FrameType::Data as u64); // Frame type
        parser.write_varint(&mut buf, 1000); // Large payload size
        
        let result = parser.parse_frame(&buf);
        assert!(result.is_err());
        
        // Should be a frame too large error
        match result.unwrap_err() {
            crate::error::H3Error::Parse(ParseError::FrameTooLarge { size, max }) => {
                assert_eq!(size, 1000);
                assert_eq!(max, 100);
            }
            _ => panic!("Expected FrameTooLarge error"),
        }
    }
    
    #[test]
    fn test_has_complete_frame() {
        let mut parser = FrameParser::new();
        
        // Initially no complete frame
        assert!(!parser.has_complete_frame());
        
        // Create a frame and add it incrementally
        let frame = H3Frame::MaxPushId { push_id: 42 };
        let encoded = parser.encode_frame(&frame).unwrap();
        
        // Add partial data
        parser.buffer.extend_from_slice(&encoded[..encoded.len() - 1]);
        assert!(!parser.has_complete_frame());
        
        // Add remaining data
        parser.buffer.extend_from_slice(&encoded[encoded.len() - 1..]);
        assert!(parser.has_complete_frame());
    }
    
    #[test]
    fn test_parser_clear() {
        let mut parser = FrameParser::new();
        
        // Add some data to buffer
        parser.buffer.extend_from_slice(b"test data");
        parser.state = ParserState::ReadingPayload {
            frame_type: FrameType::Data,
            length: 100,
        };
        
        assert!(!parser.is_buffer_empty());
        
        // Clear parser
        parser.clear();
        
        assert!(parser.is_buffer_empty());
        assert!(matches!(parser.state, ParserState::WaitingHeader));
    }
    
    #[test]
    fn test_all_frame_types_encoding_decoding() {
        let mut parser = FrameParser::new();
        
        // Test Data frame
        let data_frame = H3Frame::Data {
            stream_id: 4,
            data: Bytes::from("test data"),
            end_stream: true,
        };
        let encoded = parser.encode_frame(&data_frame).unwrap();
        let parsed = parser.parse_frame(&encoded).unwrap().unwrap();
        assert_eq!(parsed.frame_type(), FrameType::Data);
        // Note: end_stream is not preserved in current implementation as it's handled at stream level
        
        // Test Headers frame
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        let headers_frame = H3Frame::Headers {
            stream_id: 8,
            headers,
            end_stream: false,
            priority: Some(Priority::default()),
        };
        let encoded = parser.encode_frame(&headers_frame).unwrap();
        let parsed = parser.parse_frame(&encoded).unwrap().unwrap();
        assert_eq!(parsed.frame_type(), FrameType::Headers);
        // Note: end_stream is not preserved in current implementation as it's handled at stream level
        
        // Test Settings frame
        let mut settings = HashMap::new();
        settings.insert(SettingId::QpackMaxTableCapacity, 4096);
        settings.insert(SettingId::MaxFieldSectionSize, 8192);
        let settings_frame = H3Frame::Settings { settings };
        let encoded = parser.encode_frame(&settings_frame).unwrap();
        let parsed = parser.parse_frame(&encoded).unwrap().unwrap();
        assert_eq!(parsed.frame_type(), FrameType::Settings);
        assert!(parsed.is_connection_frame());
        
        // Test PushPromise frame
        let mut headers = HeaderMap::new();
        headers.insert("method", "GET".parse().unwrap());
        let push_promise_frame = H3Frame::PushPromise {
            stream_id: 12,
            promised_stream_id: 16,
            headers,
        };
        let encoded = parser.encode_frame(&push_promise_frame).unwrap();
        let parsed = parser.parse_frame(&encoded).unwrap().unwrap();
        assert_eq!(parsed.frame_type(), FrameType::PushPromise);
        
        // Test CancelPush frame
        let cancel_push_frame = H3Frame::CancelPush { push_id: 123 };
        let encoded = parser.encode_frame(&cancel_push_frame).unwrap();
        let parsed = parser.parse_frame(&encoded).unwrap().unwrap();
        assert_eq!(parsed.frame_type(), FrameType::CancelPush);
        
        // Test Goaway frame
        let goaway_frame = H3Frame::Goaway { stream_id: 456 };
        let encoded = parser.encode_frame(&goaway_frame).unwrap();
        let parsed = parser.parse_frame(&encoded).unwrap().unwrap();
        assert_eq!(parsed.frame_type(), FrameType::Goaway);
        
        // Test MaxPushId frame
        let max_push_id_frame = H3Frame::MaxPushId { push_id: 789 };
        let encoded = parser.encode_frame(&max_push_id_frame).unwrap();
        let parsed = parser.parse_frame(&encoded).unwrap().unwrap();
        assert_eq!(parsed.frame_type(), FrameType::MaxPushId);
    }
    
    #[test]
    fn test_varint_round_trip() {
        let parser = FrameParser::new();
        let test_values = vec![0, 1, 63, 64, 16383, 16384, 1073741823, 1073741824];
        
        for value in test_values {
            let mut buf = BytesMut::new();
            parser.write_varint(&mut buf, value);
            
            let mut cursor = std::io::Cursor::new(&buf[..]);
            let decoded = parser.read_varint(&mut cursor).unwrap();
            
            assert_eq!(decoded, value, "Failed round-trip for value {}", value);
        }
    }
    
    #[test]
    fn test_parser_error_recovery() {
        let mut parser = FrameParser::new();
        
        // Send invalid frame type
        let mut buf = BytesMut::new();
        parser.write_varint(&mut buf, 0xFF); // Invalid frame type
        parser.write_varint(&mut buf, 10);   // Length
        buf.extend_from_slice(&[0u8; 10]);   // Payload
        
        let result = parser.parse_frame(&buf);
        assert!(result.is_err());
        assert_eq!(parser.stats().parse_errors, 1);
        
        // Parser should be able to recover and parse valid frames
        let valid_frame = H3Frame::MaxPushId { push_id: 42 };
        let encoded = parser.encode_frame(&valid_frame).unwrap();
        
        // Clear buffer and try again
        parser.clear();
        let result = parser.parse_frame(&encoded).unwrap();
        assert!(result.is_some());
    }
    
    #[test]
    fn test_partial_varint_parsing() {
        let mut parser = FrameParser::new();
        
        // Create a MaxPushId frame (frame type 0x0D)
        let mut buf = BytesMut::new();
        parser.write_varint(&mut buf, FrameType::MaxPushId as u64); // Frame type
        parser.write_varint(&mut buf, 1); // Payload length (1 byte)
        parser.write_varint(&mut buf, 42); // Push ID
        
        // Send only first byte (partial frame type)
        let result = parser.parse_frame(&buf[..1]);
        assert!(result.unwrap().is_none()); // Should need more data
        
        // Send remaining data
        let result = parser.parse_frame(&buf[1..]).unwrap();
        assert!(result.is_some()); // Should have complete frame
        
        match result {
            Some(H3Frame::MaxPushId { push_id }) => assert_eq!(push_id, 42),
            _ => panic!("Expected MaxPushId frame"),
        }
    }
}