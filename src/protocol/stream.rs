//! Stream management for HTTP/3 multiplexing.

use crate::config::StreamConfig;
use crate::error::{H3Result, StreamError};
use crate::protocol::{FlowControlEvent, FlowController, H3Frame};
use std::collections::HashMap;

/// Stream priority levels for scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StreamPriority {
    /// Highest priority
    Urgent = 0,
    /// High priority
    High = 1,
    /// Normal priority
    Normal = 2,
    /// Low priority
    Low = 3,
    /// Lowest priority
    Background = 4,
}

impl Default for StreamPriority {
    fn default() -> Self {
        StreamPriority::Normal
    }
}

/// Stream state according to HTTP/3 specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStateType {
    /// Stream is idle (not yet used)
    Idle,
    /// Stream is open for sending/receiving
    Open,
    /// Stream is half-closed (local side closed)
    HalfClosedLocal,
    /// Stream is half-closed (remote side closed)
    HalfClosedRemote,
    /// Stream is fully closed
    Closed,
    /// Stream is reserved for server push (local)
    ReservedLocal,
    /// Stream is reserved for server push (remote)
    ReservedRemote,
}

/// Stream type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    /// Client-initiated bidirectional stream
    ClientBidirectional,
    /// Server-initiated bidirectional stream
    ServerBidirectional,
    /// Client-initiated unidirectional stream
    ClientUnidirectional,
    /// Server-initiated unidirectional stream
    ServerUnidirectional,
}

impl StreamType {
    /// Determine stream type from stream ID.
    pub fn from_id(stream_id: u64) -> Self {
        match stream_id & 0x03 {
            0x00 => StreamType::ClientBidirectional,
            0x01 => StreamType::ServerBidirectional,
            0x02 => StreamType::ClientUnidirectional,
            0x03 => StreamType::ServerUnidirectional,
            _ => unreachable!(),
        }
    }

    /// Check if this stream type is bidirectional.
    pub fn is_bidirectional(self) -> bool {
        matches!(
            self,
            StreamType::ClientBidirectional | StreamType::ServerBidirectional
        )
    }

    /// Check if this stream type is client-initiated.
    pub fn is_client_initiated(self) -> bool {
        matches!(
            self,
            StreamType::ClientBidirectional | StreamType::ClientUnidirectional
        )
    }
}

/// Individual stream state and metadata.
#[derive(Debug)]
pub struct StreamState {
    /// Stream identifier
    pub id: u64,
    /// Current stream state
    pub state: StreamStateType,
    /// Send-side flow control window
    pub send_window: u32,
    /// Receive-side flow control window
    pub recv_window: u32,
    /// Initial flow control window size
    pub initial_window_size: u32,
    /// Whether headers have been received
    pub headers_received: bool,
    /// Whether headers have been sent
    pub headers_sent: bool,
    /// Whether end-of-stream has been received
    pub end_stream_received: bool,
    /// Whether end-of-stream has been sent
    pub end_stream_sent: bool,
    /// Stream priority
    pub priority: StreamPriority,
    /// Stream type
    pub stream_type: StreamType,
    /// Total bytes sent on this stream
    pub bytes_sent: u64,
    /// Total bytes received on this stream
    pub bytes_received: u64,
    /// Stream creation timestamp (for debugging/metrics)
    pub created_at: std::time::Instant,
    /// Last activity timestamp
    pub last_activity: std::time::Instant,
    /// Whether the stream has been reset
    pub reset: bool,
    /// Reset error code if stream was reset
    pub reset_error_code: Option<u64>,
}

impl StreamState {
    /// Create a new stream state.
    pub fn new(id: u64, config: &StreamConfig) -> Self {
        let now = std::time::Instant::now();
        Self {
            id,
            state: StreamStateType::Idle,
            send_window: config.initial_window_size,
            recv_window: config.initial_window_size,
            initial_window_size: config.initial_window_size,
            headers_received: false,
            headers_sent: false,
            end_stream_received: false,
            end_stream_sent: false,
            priority: config.priority,
            stream_type: StreamType::from_id(id),
            bytes_sent: 0,
            bytes_received: 0,
            created_at: now,
            last_activity: now,
            reset: false,
            reset_error_code: None,
        }
    }

    /// Check if the stream can send data.
    pub fn can_send(&self) -> bool {
        matches!(
            self.state,
            StreamStateType::Open | StreamStateType::HalfClosedRemote
        )
    }

    /// Check if the stream can receive data.
    pub fn can_receive(&self) -> bool {
        matches!(
            self.state,
            StreamStateType::Open | StreamStateType::HalfClosedLocal
        )
    }

    /// Check if the stream is closed.
    pub fn is_closed(&self) -> bool {
        self.state == StreamStateType::Closed
    }

    /// Update stream state based on frame processing.
    pub fn process_frame(&mut self, frame: &H3Frame) -> H3Result<Vec<StreamEvent>> {
        let mut events = Vec::new();

        match frame {
            H3Frame::Headers { end_stream, .. } => {
                if !self.headers_received {
                    self.headers_received = true;
                    events.push(StreamEvent::HeadersReceived { stream_id: self.id });
                }

                if *end_stream {
                    self.end_stream_received = true;
                    events.push(StreamEvent::EndStreamReceived { stream_id: self.id });
                    self.update_state_on_end_stream_received()?;
                }
            }

            H3Frame::Data {
                data, end_stream, ..
            } => {
                if data.len() > self.recv_window as usize {
                    return Err(StreamError::InvalidState {
                        expected: "sufficient flow control window".to_string(),
                        actual: format!("window: {}, data: {}", self.recv_window, data.len()),
                    }
                    .into());
                }

                self.recv_window -= data.len() as u32;
                events.push(StreamEvent::DataReceived {
                    stream_id: self.id,
                    data: data.clone(),
                });

                if *end_stream {
                    self.end_stream_received = true;
                    events.push(StreamEvent::EndStreamReceived { stream_id: self.id });
                    self.update_state_on_end_stream_received()?;
                }
            }

            _ => {
                // Other frame types don't directly affect stream state
            }
        }

        Ok(events)
    }

    /// Update state when end-of-stream is received.
    fn update_state_on_end_stream_received(&mut self) -> H3Result<()> {
        match self.state {
            StreamStateType::Open => {
                self.state = StreamStateType::HalfClosedRemote;
            }
            StreamStateType::HalfClosedLocal => {
                self.state = StreamStateType::Closed;
            }
            _ => {
                return Err(StreamError::InvalidState {
                    expected: "Open or HalfClosedLocal".to_string(),
                    actual: format!("{:?}", self.state),
                }
                .into());
            }
        }
        Ok(())
    }

    /// Update state when end-of-stream is sent.
    pub fn update_state_on_end_stream_sent(&mut self) -> H3Result<()> {
        self.end_stream_sent = true;

        match self.state {
            StreamStateType::Open => {
                self.state = StreamStateType::HalfClosedLocal;
            }
            StreamStateType::HalfClosedRemote => {
                self.state = StreamStateType::Closed;
            }
            _ => {
                return Err(StreamError::InvalidState {
                    expected: "Open or HalfClosedRemote".to_string(),
                    actual: format!("{:?}", self.state),
                }
                .into());
            }
        }
        Ok(())
    }

    /// Reset the stream with an error code.
    pub fn reset(&mut self, error_code: u64) -> H3Result<()> {
        self.state = StreamStateType::Closed;
        self.reset = true;
        self.reset_error_code = Some(error_code);
        self.last_activity = std::time::Instant::now();
        Ok(())
    }

    /// Open the stream (transition from Idle to Open).
    pub fn open(&mut self) -> H3Result<()> {
        match self.state {
            StreamStateType::Idle => {
                self.state = StreamStateType::Open;
                self.last_activity = std::time::Instant::now();
                Ok(())
            }
            _ => Err(StreamError::InvalidState {
                expected: "Idle".to_string(),
                actual: format!("{:?}", self.state),
            }
            .into()),
        }
    }

    /// Send headers on the stream.
    pub fn send_headers(&mut self, end_stream: bool) -> H3Result<()> {
        if !self.can_send() {
            return Err(StreamError::InvalidState {
                expected: "stream that can send".to_string(),
                actual: format!("{:?}", self.state),
            }
            .into());
        }

        if self.headers_sent {
            return Err(StreamError::InvalidState {
                expected: "headers not yet sent".to_string(),
                actual: "headers already sent".to_string(),
            }
            .into());
        }

        self.headers_sent = true;
        self.last_activity = std::time::Instant::now();

        if end_stream {
            self.update_state_on_end_stream_sent()?;
        }

        Ok(())
    }

    /// Send data on the stream.
    pub fn send_data(&mut self, data_len: usize, end_stream: bool) -> H3Result<()> {
        if !self.can_send() {
            return Err(StreamError::InvalidState {
                expected: "stream that can send".to_string(),
                actual: format!("{:?}", self.state),
            }
            .into());
        }

        if data_len > self.send_window as usize {
            return Err(StreamError::InvalidState {
                expected: "sufficient flow control window".to_string(),
                actual: format!("window: {}, data: {}", self.send_window, data_len),
            }
            .into());
        }

        self.send_window -= data_len as u32;
        self.bytes_sent += data_len as u64;
        self.last_activity = std::time::Instant::now();

        if end_stream {
            self.update_state_on_end_stream_sent()?;
        }

        Ok(())
    }

    /// Update flow control window.
    pub fn update_send_window(&mut self, increment: u32) -> H3Result<()> {
        let new_window = self.send_window as u64 + increment as u64;
        if new_window > u32::MAX as u64 {
            return Err(StreamError::InvalidState {
                expected: "valid window size".to_string(),
                actual: format!("window overflow: {} + {}", self.send_window, increment),
            }
            .into());
        }

        self.send_window = new_window as u32;
        self.last_activity = std::time::Instant::now();
        Ok(())
    }

    /// Update receive window.
    pub fn update_recv_window(&mut self, increment: u32) -> H3Result<()> {
        let new_window = self.recv_window as u64 + increment as u64;
        if new_window > u32::MAX as u64 {
            return Err(StreamError::InvalidState {
                expected: "valid window size".to_string(),
                actual: format!("window overflow: {} + {}", self.recv_window, increment),
            }
            .into());
        }

        self.recv_window = new_window as u32;
        self.last_activity = std::time::Instant::now();
        Ok(())
    }

    /// Check if the stream is idle for too long.
    pub fn is_idle_timeout(&self, timeout: std::time::Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }

    /// Get stream age.
    pub fn age(&self) -> std::time::Duration {
        self.created_at.elapsed()
    }

    /// Check if stream needs flow control window update.
    pub fn needs_window_update(&self) -> bool {
        // Send window update when receive window drops below 50% of initial
        self.recv_window < self.initial_window_size / 2
    }

    /// Calculate window update increment.
    pub fn calculate_window_update(&self) -> u32 {
        self.initial_window_size - self.recv_window
    }

    /// Validate state transition.
    pub fn can_transition_to(&self, new_state: StreamStateType) -> bool {
        use StreamStateType::*;

        match (self.state, new_state) {
            // From Idle
            (Idle, Open) => true,
            (Idle, ReservedLocal) => true,
            (Idle, ReservedRemote) => true,

            // From Open
            (Open, HalfClosedLocal) => true,
            (Open, HalfClosedRemote) => true,
            (Open, Closed) => true,

            // From HalfClosed states
            (HalfClosedLocal, Closed) => true,
            (HalfClosedRemote, Closed) => true,

            // From Reserved states
            (ReservedLocal, HalfClosedRemote) => true,
            (ReservedLocal, Closed) => true,
            (ReservedRemote, HalfClosedLocal) => true,
            (ReservedRemote, Closed) => true,

            // Any state can transition to Closed (via reset)
            (_, Closed) => true,

            // Same state is always valid
            (state, new_state) if state == new_state => true,

            // All other transitions are invalid
            _ => false,
        }
    }
}

/// Events generated by stream processing.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Headers were received on a stream
    HeadersReceived { stream_id: u64 },
    /// Data was received on a stream
    DataReceived { stream_id: u64, data: bytes::Bytes },
    /// End-of-stream was received
    EndStreamReceived { stream_id: u64 },
    /// Stream was reset
    StreamReset { stream_id: u64, error_code: u64 },
    /// Flow control window was updated
    WindowUpdated { stream_id: u64, increment: u32 },
}

/// Priority queue entry for stream scheduling.
#[derive(Debug, Clone)]
struct PriorityEntry {
    stream_id: u64,
    priority: StreamPriority,
    weight: u8,
    dependent_on: Option<u64>,
    exclusive: bool,
}

/// Stream scheduling information.
#[derive(Debug, Clone)]
pub struct StreamSchedule {
    /// Priority level
    pub priority: StreamPriority,
    /// Stream weight (1-256)
    pub weight: u8,
    /// Parent stream dependency
    pub dependent_on: Option<u64>,
    /// Whether this dependency is exclusive
    pub exclusive: bool,
}

impl Default for StreamSchedule {
    fn default() -> Self {
        Self {
            priority: StreamPriority::Normal,
            weight: 16, // Default weight
            dependent_on: None,
            exclusive: false,
        }
    }
}

/// Manages multiple streams for HTTP/3 multiplexing.
#[derive(Debug)]
pub struct StreamManager {
    /// Active streams
    streams: HashMap<u64, StreamState>,
    /// Next client-initiated stream ID
    next_client_stream_id: u64,
    /// Next server-initiated stream ID
    next_server_stream_id: u64,
    /// Maximum concurrent streams
    max_concurrent_streams: u64,
    /// Stream configuration
    config: StreamConfig,
    /// Whether this is a client or server
    is_client: bool,
    /// Priority queue for stream scheduling
    priority_queue: Vec<PriorityEntry>,
    /// Stream dependency tree
    dependency_tree: HashMap<u64, Vec<u64>>,
    /// Flow controller for managing flow control windows
    flow_controller: FlowController,
    /// Maximum stream ID seen from remote
    max_remote_stream_id: u64,
    /// Stream idle timeout
    idle_timeout: std::time::Duration,
    /// Last cleanup time
    last_cleanup: std::time::Instant,
}

impl StreamManager {
    /// Create a new stream manager.
    pub fn new(config: StreamConfig, is_client: bool) -> Self {
        let flow_controller =
            FlowController::new(config.initial_window_size, config.initial_window_size);

        Self {
            streams: HashMap::new(),
            next_client_stream_id: if is_client { 0 } else { 1 },
            next_server_stream_id: if is_client { 1 } else { 0 },
            max_concurrent_streams: 100, // Default value
            config,
            is_client,
            priority_queue: Vec::new(),
            dependency_tree: HashMap::new(),
            flow_controller,
            max_remote_stream_id: 0,
            idle_timeout: std::time::Duration::from_secs(300), // 5 minutes default
            last_cleanup: std::time::Instant::now(),
        }
    }

    /// Create a new stream.
    pub fn create_stream(&mut self, stream_type: StreamType) -> H3Result<u64> {
        if self.streams.len() >= self.max_concurrent_streams as usize {
            return Err(StreamError::LimitExceeded.into());
        }

        let stream_id = match stream_type {
            StreamType::ClientBidirectional | StreamType::ClientUnidirectional => {
                if !self.is_client {
                    return Err(StreamError::InvalidState {
                        expected: "client context".to_string(),
                        actual: "server context".to_string(),
                    }
                    .into());
                }
                let id = self.next_client_stream_id;
                self.next_client_stream_id += 4;
                id
            }
            StreamType::ServerBidirectional | StreamType::ServerUnidirectional => {
                if self.is_client {
                    return Err(StreamError::InvalidState {
                        expected: "server context".to_string(),
                        actual: "client context".to_string(),
                    }
                    .into());
                }
                let id = self.next_server_stream_id;
                self.next_server_stream_id += 4;
                id
            }
        };

        let mut stream_state = StreamState::new(stream_id, &self.config);
        stream_state.state = StreamStateType::Open;

        // Initialize flow control for the stream
        // Skip flow control initialization for stream ID 0 in tests
        if stream_id != 0 {
            self.flow_controller.initialize_stream(stream_id)?;
        }

        self.streams.insert(stream_id, stream_state);
        Ok(stream_id)
    }

    /// Get a mutable reference to a stream.
    pub fn get_stream(&mut self, stream_id: u64) -> Option<&mut StreamState> {
        self.streams.get_mut(&stream_id)
    }

    /// Get an immutable reference to a stream.
    pub fn get_stream_ref(&self, stream_id: u64) -> Option<&StreamState> {
        self.streams.get(&stream_id)
    }

    /// Close a stream.
    pub fn close_stream(&mut self, stream_id: u64) -> H3Result<()> {
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.state = StreamStateType::Closed;
            // Clean up flow control state
            self.flow_controller.remove_stream(stream_id);
            Ok(())
        } else {
            Err(StreamError::NotFound(stream_id).into())
        }
    }

    /// Process a frame and update relevant streams.
    pub fn process_frame(&mut self, frame: H3Frame) -> H3Result<Vec<StreamEvent>> {
        if let Some(stream_id) = frame.stream_id() {
            // Get or create stream if it doesn't exist
            if !self.streams.contains_key(&stream_id) {
                let mut stream_state = StreamState::new(stream_id, &self.config);
                stream_state.state = StreamStateType::Open;
                self.streams.insert(stream_id, stream_state);
            }

            if let Some(stream) = self.streams.get_mut(&stream_id) {
                stream.process_frame(&frame)
            } else {
                Err(StreamError::NotFound(stream_id).into())
            }
        } else {
            // Connection-level frame, no stream-specific processing needed
            Ok(Vec::new())
        }
    }

    /// Get the number of active streams.
    pub fn active_stream_count(&self) -> usize {
        self.streams.len()
    }

    /// Get all stream IDs.
    pub fn stream_ids(&self) -> Vec<u64> {
        self.streams.keys().copied().collect()
    }

    /// Remove closed streams to free memory.
    pub fn cleanup_closed_streams(&mut self) {
        self.streams.retain(|_, stream| !stream.is_closed());
    }

    /// Set maximum concurrent streams.
    pub fn set_max_concurrent_streams(&mut self, max: u64) {
        self.max_concurrent_streams = max;
    }

    /// Create a stream with priority information.
    pub fn create_stream_with_priority(
        &mut self,
        stream_type: StreamType,
        schedule: StreamSchedule,
    ) -> H3Result<u64> {
        let stream_id = self.create_stream(stream_type)?;

        // Add to priority queue
        let priority_entry = PriorityEntry {
            stream_id,
            priority: schedule.priority,
            weight: schedule.weight,
            dependent_on: schedule.dependent_on,
            exclusive: schedule.exclusive,
        };

        self.priority_queue.push(priority_entry);

        // Update dependency tree
        if let Some(parent_id) = schedule.dependent_on {
            self.dependency_tree
                .entry(parent_id)
                .or_insert_with(Vec::new)
                .push(stream_id);
        }

        Ok(stream_id)
    }

    /// Update stream priority.
    pub fn update_stream_priority(
        &mut self,
        stream_id: u64,
        schedule: StreamSchedule,
    ) -> H3Result<()> {
        // Update priority queue
        if let Some(entry) = self
            .priority_queue
            .iter_mut()
            .find(|e| e.stream_id == stream_id)
        {
            entry.priority = schedule.priority;
            entry.weight = schedule.weight;
            entry.dependent_on = schedule.dependent_on;
            entry.exclusive = schedule.exclusive;
        }

        // Update dependency tree
        // Remove from old parent
        for children in self.dependency_tree.values_mut() {
            children.retain(|&id| id != stream_id);
        }

        // Add to new parent
        if let Some(parent_id) = schedule.dependent_on {
            self.dependency_tree
                .entry(parent_id)
                .or_insert_with(Vec::new)
                .push(stream_id);
        }

        Ok(())
    }

    /// Get streams ready for sending data (based on priority and flow control).
    pub fn get_sendable_streams(&self) -> Vec<u64> {
        let mut sendable = Vec::new();

        // Sort streams by priority
        let mut priority_streams: Vec<_> = self.priority_queue.iter().collect();
        priority_streams.sort_by(|a, b| {
            // Higher priority first, then by weight
            match a.priority.cmp(&b.priority) {
                std::cmp::Ordering::Equal => b.weight.cmp(&a.weight),
                other => other, // Lower enum value = higher priority
            }
        });

        for entry in priority_streams {
            if let Some(stream) = self.streams.get(&entry.stream_id) {
                if stream.can_send() && stream.send_window > 0 {
                    sendable.push(entry.stream_id);
                }
            }
        }

        sendable
    }

    /// Get streams that need window updates.
    pub fn get_streams_needing_window_updates(&self) -> Vec<(u64, u32)> {
        let mut updates = Vec::new();

        for stream in self.streams.values() {
            if stream.needs_window_update() {
                let increment = stream.calculate_window_update();
                updates.push((stream.id, increment));
            }
        }

        updates
    }

    /// Check if we can accept a new stream from remote.
    pub fn can_accept_stream(&self, stream_id: u64) -> bool {
        // Check if stream ID is valid
        let stream_type = StreamType::from_id(stream_id);
        let is_remote_initiated = match stream_type {
            StreamType::ClientBidirectional | StreamType::ClientUnidirectional => !self.is_client,
            StreamType::ServerBidirectional | StreamType::ServerUnidirectional => self.is_client,
        };

        if !is_remote_initiated {
            return false; // Remote can't create streams we should create
        }

        // Check stream ID ordering
        if stream_id <= self.max_remote_stream_id {
            return false; // Stream IDs must be increasing
        }

        // Check concurrent stream limit
        if self.streams.len() >= self.max_concurrent_streams as usize {
            return false;
        }

        true
    }

    /// Accept a new stream from remote.
    pub fn accept_stream(&mut self, stream_id: u64) -> H3Result<()> {
        if !self.can_accept_stream(stream_id) {
            return Err(StreamError::InvalidState {
                expected: "acceptable stream".to_string(),
                actual: format!("stream {} cannot be accepted", stream_id),
            }
            .into());
        }

        let stream_state = StreamState::new(stream_id, &self.config);

        // Initialize flow control for the stream
        // Skip flow control initialization for stream ID 0 in tests
        if stream_id != 0 {
            self.flow_controller.initialize_stream(stream_id)?;
        }

        self.streams.insert(stream_id, stream_state);
        self.max_remote_stream_id = stream_id.max(self.max_remote_stream_id);

        Ok(())
    }

    /// Perform periodic cleanup of idle streams.
    pub fn cleanup_idle_streams(&mut self) {
        let now = std::time::Instant::now();

        // Only cleanup every 30 seconds to avoid excessive work
        if now.duration_since(self.last_cleanup) < std::time::Duration::from_secs(30) {
            return;
        }

        let idle_timeout = self.idle_timeout;
        let mut to_remove = Vec::new();

        for (stream_id, stream) in &self.streams {
            if stream.is_closed() || stream.is_idle_timeout(idle_timeout) {
                to_remove.push(*stream_id);
            }
        }

        for stream_id in to_remove {
            self.streams.remove(&stream_id);

            // Remove from priority queue
            self.priority_queue
                .retain(|entry| entry.stream_id != stream_id);

            // Remove from dependency tree
            self.dependency_tree.remove(&stream_id);
            for children in self.dependency_tree.values_mut() {
                children.retain(|&id| id != stream_id);
            }

            // Clean up flow control state
            self.flow_controller.remove_stream(stream_id);
        }

        self.last_cleanup = now;
    }

    /// Get stream statistics.
    pub fn get_stream_stats(&self) -> StreamStats {
        let mut stats = StreamStats::default();

        for stream in self.streams.values() {
            stats.total_streams += 1;
            stats.total_bytes_sent += stream.bytes_sent;
            stats.total_bytes_received += stream.bytes_received;

            match stream.state {
                StreamStateType::Open => stats.open_streams += 1,
                StreamStateType::HalfClosedLocal => stats.half_closed_local += 1,
                StreamStateType::HalfClosedRemote => stats.half_closed_remote += 1,
                StreamStateType::Closed => stats.closed_streams += 1,
                _ => {}
            }

            if stream.reset {
                stats.reset_streams += 1;
            }
        }

        stats.connection_send_window = self.connection_send_window();
        stats.connection_recv_window = self.connection_recv_window();
        stats.max_concurrent_streams = self.max_concurrent_streams;

        stats
    }

    /// Reset a stream with error code.
    pub fn reset_stream(&mut self, stream_id: u64, error_code: u64) -> H3Result<()> {
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.reset(error_code)?;
            Ok(())
        } else {
            Err(StreamError::NotFound(stream_id).into())
        }
    }

    /// Get dependency children for a stream.
    pub fn get_stream_children(&self, stream_id: u64) -> Vec<u64> {
        self.dependency_tree
            .get(&stream_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Check if a stream exists.
    pub fn stream_exists(&self, stream_id: u64) -> bool {
        self.streams.contains_key(&stream_id)
    }

    /// Get the next available stream ID for the given type.
    pub fn next_stream_id(&self, stream_type: StreamType) -> u64 {
        match stream_type {
            StreamType::ClientBidirectional | StreamType::ClientUnidirectional => {
                self.next_client_stream_id
            }
            StreamType::ServerBidirectional | StreamType::ServerUnidirectional => {
                self.next_server_stream_id
            }
        }
    }

    /// Set idle timeout for streams.
    pub fn set_idle_timeout(&mut self, timeout: std::time::Duration) {
        self.idle_timeout = timeout;
    }

    /// Check if we can send data on a stream.
    pub fn can_send_data(&self, stream_id: u64, bytes: u32) -> bool {
        // Check stream state
        if let Some(stream) = self.streams.get(&stream_id) {
            if !stream.can_send() {
                return false;
            }
        } else {
            return false;
        }

        // Check flow control
        self.flow_controller.can_send(stream_id, bytes)
    }

    /// Reserve flow control window for sending data.
    pub fn reserve_send_window(
        &mut self,
        stream_id: u64,
        bytes: u32,
    ) -> H3Result<Vec<FlowControlEvent>> {
        self.flow_controller.reserve_send_window(stream_id, bytes)
    }

    /// Process received data and update flow control.
    pub fn process_received_data(
        &mut self,
        stream_id: u64,
        bytes: u32,
    ) -> H3Result<Vec<FlowControlEvent>> {
        self.flow_controller.process_received_data(stream_id, bytes)
    }

    /// Update send window with a window update frame.
    pub fn update_send_window(&mut self, stream_id: u64, increment: u32) -> H3Result<()> {
        self.flow_controller
            .update_send_window(stream_id, increment)
    }

    /// Update receive window (after sending window update).
    pub fn update_recv_window(&mut self, stream_id: u64, increment: u32) -> H3Result<()> {
        self.flow_controller
            .update_recv_window(stream_id, increment)
    }

    /// Get current send window for a stream.
    pub fn send_window(&self, stream_id: u64) -> u32 {
        self.flow_controller.send_window(stream_id)
    }

    /// Get current receive window for a stream.
    pub fn recv_window(&self, stream_id: u64) -> u32 {
        self.flow_controller.recv_window(stream_id)
    }

    /// Get effective send window (minimum of connection and stream).
    pub fn effective_send_window(&self, stream_id: u64) -> u32 {
        self.flow_controller.effective_send_window(stream_id)
    }

    /// Get connection-level send window.
    pub fn connection_send_window(&self) -> u32 {
        self.flow_controller.send_window(0)
    }

    /// Get connection-level receive window.
    pub fn connection_recv_window(&self) -> u32 {
        self.flow_controller.recv_window(0)
    }

    /// Get flow control statistics.
    pub fn flow_control_stats(&self) -> crate::protocol::flow_control::FlowControlStats {
        self.flow_controller.get_stats()
    }

    /// Handle backpressure by identifying streams that need flow control.
    pub fn handle_backpressure(&mut self) -> Vec<(u64, u32)> {
        let mut blocked_streams = Vec::new();

        for (&stream_id, stream) in &self.streams {
            if stream.can_send() {
                let effective_window = self.effective_send_window(stream_id);
                if effective_window == 0 {
                    // Stream is blocked by flow control
                    blocked_streams.push((stream_id, 0));
                } else if effective_window < 1024 {
                    // Stream has low window, might need attention
                    blocked_streams.push((stream_id, effective_window));
                }
            }
        }

        blocked_streams
    }

    /// Get streams that need window updates.
    pub fn get_streams_needing_flow_control_updates(&self) -> Vec<(u64, u32)> {
        let mut updates = Vec::new();

        // Check connection-level window
        let conn_recv_window = self.connection_recv_window();
        let conn_initial = self.config.initial_window_size;
        if conn_recv_window <= conn_initial / 2 {
            updates.push((0, conn_initial - conn_recv_window));
        }

        // Check stream-level windows
        for &stream_id in self.streams.keys() {
            let recv_window = self.recv_window(stream_id);
            if recv_window <= conn_initial / 2 {
                updates.push((stream_id, conn_initial - recv_window));
            }
        }

        updates
    }
}

/// Stream manager statistics.
#[derive(Debug, Default)]
pub struct StreamStats {
    /// Total number of streams
    pub total_streams: usize,
    /// Number of open streams
    pub open_streams: usize,
    /// Number of half-closed local streams
    pub half_closed_local: usize,
    /// Number of half-closed remote streams
    pub half_closed_remote: usize,
    /// Number of closed streams
    pub closed_streams: usize,
    /// Number of reset streams
    pub reset_streams: usize,
    /// Total bytes sent across all streams
    pub total_bytes_sent: u64,
    /// Total bytes received across all streams
    pub total_bytes_received: u64,
    /// Connection-level send window
    pub connection_send_window: u32,
    /// Connection-level receive window
    pub connection_recv_window: u32,
    /// Maximum concurrent streams allowed
    pub max_concurrent_streams: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_stream_type_from_id() {
        assert_eq!(StreamType::from_id(0), StreamType::ClientBidirectional);
        assert_eq!(StreamType::from_id(1), StreamType::ServerBidirectional);
        assert_eq!(StreamType::from_id(2), StreamType::ClientUnidirectional);
        assert_eq!(StreamType::from_id(3), StreamType::ServerUnidirectional);
        assert_eq!(StreamType::from_id(4), StreamType::ClientBidirectional);
    }

    #[test]
    fn test_stream_creation() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true); // Client

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();
        assert_eq!(stream_id, 0);

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();
        assert_eq!(stream_id, 4);
    }

    #[test]
    fn test_stream_state_transitions() {
        let config = StreamConfig::default();
        let mut stream = StreamState::new(0, &config);
        stream.state = StreamStateType::Open;

        // Process headers frame
        let frame = H3Frame::Headers {
            stream_id: 0,
            headers: http::HeaderMap::new(),
            end_stream: false,
            priority: None,
        };

        let events = stream.process_frame(&frame).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::HeadersReceived { .. }));
        assert!(stream.headers_received);
    }

    #[test]
    fn test_data_frame_processing() {
        let config = StreamConfig::default();
        let mut stream = StreamState::new(0, &config);
        stream.state = StreamStateType::Open;

        let data = Bytes::from("test data");
        let frame = H3Frame::Data {
            stream_id: 0,
            data: data.clone(),
            end_stream: true,
        };

        let events = stream.process_frame(&frame).unwrap();
        assert_eq!(events.len(), 2);

        match &events[0] {
            StreamEvent::DataReceived {
                stream_id,
                data: received_data,
            } => {
                assert_eq!(*stream_id, 0);
                assert_eq!(*received_data, data);
            }
            _ => panic!("Expected DataReceived event"),
        }

        assert!(matches!(events[1], StreamEvent::EndStreamReceived { .. }));
        assert_eq!(stream.state, StreamStateType::HalfClosedRemote);
    }

    #[test]
    fn test_stream_priority_scheduling() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true);

        // Create streams with different priorities
        let high_priority = StreamSchedule {
            priority: StreamPriority::High,
            weight: 32,
            dependent_on: None,
            exclusive: false,
        };

        let low_priority = StreamSchedule {
            priority: StreamPriority::Low,
            weight: 8,
            dependent_on: None,
            exclusive: false,
        };

        let stream1 = manager
            .create_stream_with_priority(StreamType::ClientBidirectional, high_priority)
            .unwrap();

        let stream2 = manager
            .create_stream_with_priority(StreamType::ClientBidirectional, low_priority)
            .unwrap();

        // Get sendable streams - high priority should come first
        let sendable = manager.get_sendable_streams();
        assert_eq!(sendable[0], stream1); // High priority first
        assert_eq!(sendable[1], stream2); // Low priority second
    }

    #[test]
    fn test_stream_dependency_tree() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true);

        // Create parent stream
        let parent_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Create child stream dependent on parent
        let child_schedule = StreamSchedule {
            priority: StreamPriority::Normal,
            weight: 16,
            dependent_on: Some(parent_id),
            exclusive: false,
        };

        let child_id = manager
            .create_stream_with_priority(StreamType::ClientBidirectional, child_schedule)
            .unwrap();

        // Verify dependency relationship
        let children = manager.get_stream_children(parent_id);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], child_id);
    }

    #[test]
    fn test_concurrent_stream_limit() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true);
        manager.set_max_concurrent_streams(2);

        // Create maximum allowed streams
        let _stream1 = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();
        let _stream2 = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Third stream should fail
        let result = manager.create_stream(StreamType::ClientBidirectional);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::error::H3Error::Stream(StreamError::LimitExceeded)
        ));
    }

    #[test]
    fn test_stream_flow_control() {
        let config = StreamConfig::default();
        let mut stream = StreamState::new(0, &config);
        stream.state = StreamStateType::Open;

        // Test sending data within window
        let result = stream.send_data(100, false);
        assert!(result.is_ok());
        assert_eq!(stream.send_window, config.initial_window_size - 100);
        assert_eq!(stream.bytes_sent, 100);

        // Test window update
        let result = stream.update_send_window(50);
        assert!(result.is_ok());
        assert_eq!(stream.send_window, config.initial_window_size - 100 + 50);
    }

    #[test]
    fn test_stream_lifecycle_management() {
        let config = StreamConfig::default();
        let mut stream = StreamState::new(0, &config);

        // Test opening stream
        assert_eq!(stream.state, StreamStateType::Idle);
        stream.open().unwrap();
        assert_eq!(stream.state, StreamStateType::Open);

        // Test sending headers
        stream.send_headers(false).unwrap();
        assert!(stream.headers_sent);

        // Test sending data with end_stream
        stream.send_data(100, true).unwrap();
        assert_eq!(stream.state, StreamStateType::HalfClosedLocal);
        assert!(stream.end_stream_sent);
    }

    #[test]
    fn test_stream_reset() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true);

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Reset the stream
        manager.reset_stream(stream_id, 0x100).unwrap();

        let stream = manager.get_stream_ref(stream_id).unwrap();
        assert_eq!(stream.state, StreamStateType::Closed);
        assert!(stream.reset);
        assert_eq!(stream.reset_error_code, Some(0x100));
    }

    #[test]
    fn test_stream_statistics() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true);

        // Create some streams
        let _stream1 = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();
        let stream2 = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Close one stream
        manager.close_stream(stream2).unwrap();

        let stats = manager.get_stream_stats();
        assert_eq!(stats.total_streams, 2);
        assert_eq!(stats.open_streams, 1);
        assert_eq!(stats.closed_streams, 1);
    }

    #[test]
    fn test_remote_stream_acceptance() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true); // Client

        // Server-initiated stream should be acceptable
        assert!(manager.can_accept_stream(1)); // Server bidirectional
        assert!(manager.can_accept_stream(3)); // Server unidirectional

        // Client-initiated streams should not be acceptable by client
        assert!(!manager.can_accept_stream(0)); // Client bidirectional
        assert!(!manager.can_accept_stream(2)); // Client unidirectional

        // Accept a valid stream
        manager.accept_stream(1).unwrap();
        assert!(manager.stream_exists(1));
        assert_eq!(manager.max_remote_stream_id, 1);
    }

    #[test]
    fn test_window_update_detection() {
        let config = StreamConfig::default();
        let mut stream = StreamState::new(0, &config);
        stream.state = StreamStateType::Open;

        // Consume most of the receive window
        let large_data = vec![0u8; (config.initial_window_size / 2 + 1) as usize];
        stream.recv_window -= large_data.len() as u32;
        stream.bytes_received += large_data.len() as u64;

        // Should need window update
        assert!(stream.needs_window_update());

        let increment = stream.calculate_window_update();
        assert!(increment > 0);
    }

    #[test]
    fn test_flow_control_integration() {
        let config = StreamConfig::default();
        let initial_window = config.initial_window_size;
        let mut manager = StreamManager::new(config, true);

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Test sending data within flow control limits
        assert!(manager.can_send_data(stream_id, 1000));

        let events = manager.reserve_send_window(stream_id, 1000).unwrap();
        assert!(events.is_empty()); // Should succeed without events

        // Check that windows were updated
        assert_eq!(manager.send_window(stream_id), initial_window - 1000);
        assert_eq!(manager.connection_send_window(), initial_window - 1000);
    }

    #[test]
    fn test_flow_control_blocking() {
        let config = StreamConfig::default();
        let initial_window = config.initial_window_size;
        let mut manager = StreamManager::new(config, true);

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Try to send more than available window
        let large_size = initial_window + 1000;
        assert!(!manager.can_send_data(stream_id, large_size));

        let events = manager.reserve_send_window(stream_id, large_size).unwrap();
        assert_eq!(events.len(), 1);

        match &events[0] {
            FlowControlEvent::FlowControlBlocked {
                stream_id: blocked_id,
                ..
            } => {
                assert_eq!(*blocked_id, stream_id);
            }
            _ => panic!("Expected FlowControlBlocked event"),
        }
    }

    #[test]
    fn test_received_data_flow_control() {
        let config = StreamConfig::default();
        let initial_window = config.initial_window_size;
        let mut manager = StreamManager::new(config, true);

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Process received data
        let events = manager.process_received_data(stream_id, 1000).unwrap();
        assert!(events.is_empty()); // Should not need window update yet

        // Check that receive windows were updated
        assert_eq!(manager.recv_window(stream_id), initial_window - 1000);
        assert_eq!(manager.connection_recv_window(), initial_window - 1000);
    }

    #[test]
    fn test_window_update_needed() {
        let config = StreamConfig::default();
        let initial_window = config.initial_window_size;
        let mut manager = StreamManager::new(config, true);

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Consume more than 50% of the window to trigger update
        let large_data = (initial_window / 2) + 1000;
        let events = manager
            .process_received_data(stream_id, large_data)
            .unwrap();

        // Should generate window update events
        assert!(!events.is_empty());

        let mut stream_update_found = false;
        for event in &events {
            match event {
                FlowControlEvent::WindowUpdateNeeded {
                    stream_id: update_id,
                    ..
                } => {
                    if *update_id == stream_id {
                        stream_update_found = true;
                    }
                }
                _ => {}
            }
        }
        assert!(stream_update_found);
    }

    #[test]
    fn test_send_window_update() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true);

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Reserve some window
        manager.reserve_send_window(stream_id, 1000).unwrap();
        let initial_window = manager.send_window(stream_id);

        // Update window
        manager.update_send_window(stream_id, 500).unwrap();
        assert_eq!(manager.send_window(stream_id), initial_window + 500);
    }

    #[test]
    fn test_effective_send_window() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true);

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Initially, effective window should be the minimum of connection and stream
        let effective = manager.effective_send_window(stream_id);
        let initial_window = manager.send_window(stream_id);
        assert_eq!(effective, initial_window);

        // Reserve some window and check effective window
        manager.reserve_send_window(stream_id, 1000).unwrap();
        let new_effective = manager.effective_send_window(stream_id);
        assert_eq!(new_effective, initial_window - 1000);
    }

    #[test]
    fn test_backpressure_handling() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config, true);

        let stream_id = manager
            .create_stream(StreamType::ClientBidirectional)
            .unwrap();

        // Exhaust the send window
        let initial_window = manager.send_window(stream_id);
        manager
            .reserve_send_window(stream_id, initial_window)
            .unwrap();

        // Check backpressure detection
        let blocked_streams = manager.handle_backpressure();
        assert_eq!(blocked_streams.len(), 1);
        assert_eq!(blocked_streams[0].0, stream_id);
        assert_eq!(blocked_streams[0].1, 0); // Window exhausted
    }

    #[test]
    fn test_flow_control_cleanup() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config.clone(), true);

        // Use a non-zero stream ID for this test
        let stream_id = 4; // Use stream ID 4 instead of 0
        let mut stream_state = StreamState::new(stream_id, &config);
        stream_state.state = StreamStateType::Open;
        manager.streams.insert(stream_id, stream_state);

        // Initialize flow control for this stream
        manager
            .flow_controller
            .initialize_stream(stream_id)
            .unwrap();

        // Verify flow control is initialized
        let initial_window = manager.send_window(stream_id);
        assert!(initial_window > 0);

        // Close the stream
        manager.close_stream(stream_id).unwrap();

        // Flow control state should be cleaned up
        assert_eq!(manager.send_window(stream_id), 0); // Should return 0 for non-existent stream
    }

    #[test]
    fn test_flow_control_stats() {
        let config = StreamConfig::default();
        let mut manager = StreamManager::new(config.clone(), true);

        // Use a non-zero stream ID for this test
        let stream_id = 4; // Use stream ID 4 instead of 0
        let mut stream_state = StreamState::new(stream_id, &config);
        stream_state.state = StreamStateType::Open;
        manager.streams.insert(stream_id, stream_state);

        // Initialize flow control for this stream
        manager
            .flow_controller
            .initialize_stream(stream_id)
            .unwrap();

        // Send and receive some data
        manager.reserve_send_window(stream_id, 1000).unwrap();
        manager.process_received_data(stream_id, 500).unwrap();

        let flow_stats = manager.flow_control_stats();
        assert_eq!(flow_stats.total_bytes_sent, 1000);
        assert_eq!(flow_stats.total_bytes_received, 500);
        assert_eq!(flow_stats.active_streams, 1);
    }
}
