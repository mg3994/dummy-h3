//! Flow control implementation for HTTP/3.
//!
//! This module provides stream-level and connection-level flow control
//! mechanisms according to the HTTP/3 specification.

use crate::error::{H3Result, StreamError};
use std::collections::HashMap;

/// Flow control window size limits.
pub const MIN_WINDOW_SIZE: u32 = 1024;
pub const MAX_WINDOW_SIZE: u32 = (1 << 31) - 1; // 2^31 - 1
pub const DEFAULT_WINDOW_SIZE: u32 = 65536;

/// Flow control window update threshold (percentage).
pub const WINDOW_UPDATE_THRESHOLD: f32 = 0.5; // 50%

/// Flow controller for managing flow control windows.
#[derive(Debug)]
pub struct FlowController {
    /// Connection-level send window
    connection_send_window: u32,
    /// Connection-level receive window
    connection_recv_window: u32,
    /// Initial connection window size
    initial_connection_window: u32,
    /// Stream-level send windows
    stream_send_windows: HashMap<u64, u32>,
    /// Stream-level receive windows
    stream_recv_windows: HashMap<u64, u32>,
    /// Initial stream window size
    initial_stream_window: u32,
    /// Total bytes sent on connection
    total_bytes_sent: u64,
    /// Total bytes received on connection
    total_bytes_received: u64,
    /// Pending window updates
    pending_updates: Vec<WindowUpdate>,
}

/// Window update information.
#[derive(Debug, Clone)]
pub struct WindowUpdate {
    /// Stream ID (0 for connection-level)
    pub stream_id: u64,
    /// Window increment
    pub increment: u32,
}

/// Flow control event.
#[derive(Debug, Clone)]
pub enum FlowControlEvent {
    /// Window update needed
    WindowUpdateNeeded {
        stream_id: u64,
        increment: u32,
    },
    /// Flow control blocked
    FlowControlBlocked {
        stream_id: u64,
        available_window: u32,
        requested_bytes: u32,
    },
    /// Window exhausted
    WindowExhausted {
        stream_id: u64,
    },
}

impl FlowController {
    /// Create a new flow controller.
    pub fn new(
        initial_connection_window: u32,
        initial_stream_window: u32,
    ) -> Self {
        Self {
            connection_send_window: initial_connection_window,
            connection_recv_window: initial_connection_window,
            initial_connection_window,
            stream_send_windows: HashMap::new(),
            stream_recv_windows: HashMap::new(),
            initial_stream_window,
            total_bytes_sent: 0,
            total_bytes_received: 0,
            pending_updates: Vec::new(),
        }
    }
    
    /// Initialize flow control for a new stream.
    pub fn initialize_stream(&mut self, stream_id: u64) -> H3Result<()> {
        if stream_id == 0 {
            return Err(StreamError::InvalidState {
                expected: "non-zero stream ID".to_string(),
                actual: "stream ID 0".to_string(),
            }.into());
        }
        
        self.stream_send_windows.insert(stream_id, self.initial_stream_window);
        self.stream_recv_windows.insert(stream_id, self.initial_stream_window);
        
        Ok(())
    }
    
    /// Remove flow control state for a stream.
    pub fn remove_stream(&mut self, stream_id: u64) {
        self.stream_send_windows.remove(&stream_id);
        self.stream_recv_windows.remove(&stream_id);
    }
    
    /// Check if we can send data on a stream.
    pub fn can_send(&self, stream_id: u64, bytes: u32) -> bool {
        // Check connection-level window
        if self.connection_send_window < bytes {
            return false;
        }
        
        // Check stream-level window (if not connection-level)
        if stream_id != 0 {
            if let Some(&stream_window) = self.stream_send_windows.get(&stream_id) {
                if stream_window < bytes {
                    return false;
                }
            } else {
                return false; // Stream not initialized
            }
        }
        
        true
    }
    
    /// Reserve flow control window for sending data.
    pub fn reserve_send_window(
        &mut self,
        stream_id: u64,
        bytes: u32,
    ) -> H3Result<Vec<FlowControlEvent>> {
        let mut events = Vec::new();
        
        // Check and reserve connection-level window
        if self.connection_send_window < bytes {
            events.push(FlowControlEvent::FlowControlBlocked {
                stream_id: 0,
                available_window: self.connection_send_window,
                requested_bytes: bytes,
            });
            return Ok(events);
        }
        
        // Check and reserve stream-level window (if not connection-level)
        if stream_id != 0 {
            let stream_window = self.stream_send_windows.get(&stream_id).copied()
                .unwrap_or(0);
            
            if stream_window < bytes {
                events.push(FlowControlEvent::FlowControlBlocked {
                    stream_id,
                    available_window: stream_window,
                    requested_bytes: bytes,
                });
                return Ok(events);
            }
            
            // Reserve stream window
            self.stream_send_windows.insert(stream_id, stream_window - bytes);
            
            if stream_window - bytes == 0 {
                events.push(FlowControlEvent::WindowExhausted { stream_id });
            }
        }
        
        // Reserve connection window
        self.connection_send_window -= bytes;
        self.total_bytes_sent += bytes as u64;
        
        if self.connection_send_window == 0 {
            events.push(FlowControlEvent::WindowExhausted { stream_id: 0 });
        }
        
        Ok(events)
    }
    
    /// Process received data and update receive windows.
    pub fn process_received_data(
        &mut self,
        stream_id: u64,
        bytes: u32,
    ) -> H3Result<Vec<FlowControlEvent>> {
        let mut events = Vec::new();
        
        // Update connection-level receive window
        if self.connection_recv_window < bytes {
            return Err(StreamError::InvalidState {
                expected: "sufficient connection receive window".to_string(),
                actual: format!("window: {}, data: {}", self.connection_recv_window, bytes),
            }.into());
        }
        
        self.connection_recv_window -= bytes;
        self.total_bytes_received += bytes as u64;
        
        // Update stream-level receive window (if not connection-level)
        if stream_id != 0 {
            let stream_window = self.stream_recv_windows.get(&stream_id).copied()
                .unwrap_or(0);
            
            if stream_window < bytes {
                return Err(StreamError::InvalidState {
                    expected: "sufficient stream receive window".to_string(),
                    actual: format!("window: {}, data: {}", stream_window, bytes),
                }.into());
            }
            
            let new_stream_window = stream_window - bytes;
            self.stream_recv_windows.insert(stream_id, new_stream_window);
            
            // Check if stream window update is needed
            if self.needs_window_update(new_stream_window, self.initial_stream_window) {
                let increment = self.calculate_window_update(new_stream_window, self.initial_stream_window);
                events.push(FlowControlEvent::WindowUpdateNeeded {
                    stream_id,
                    increment,
                });
            }
        }
        
        // Check if connection window update is needed
        if self.needs_window_update(self.connection_recv_window, self.initial_connection_window) {
            let increment = self.calculate_window_update(
                self.connection_recv_window,
                self.initial_connection_window,
            );
            events.push(FlowControlEvent::WindowUpdateNeeded {
                stream_id: 0,
                increment,
            });
        }
        
        Ok(events)
    }
    
    /// Update send window with a window update frame.
    pub fn update_send_window(
        &mut self,
        stream_id: u64,
        increment: u32,
    ) -> H3Result<()> {
        if stream_id == 0 {
            // Connection-level window update
            let new_window = self.connection_send_window as u64 + increment as u64;
            if new_window > MAX_WINDOW_SIZE as u64 {
                return Err(StreamError::InvalidState {
                    expected: "valid window size".to_string(),
                    actual: format!("window overflow: {} + {}", self.connection_send_window, increment),
                }.into());
            }
            self.connection_send_window = new_window as u32;
        } else {
            // Stream-level window update
            let current_window = self.stream_send_windows.get(&stream_id).copied()
                .unwrap_or(0);
            let new_window = current_window as u64 + increment as u64;
            
            if new_window > MAX_WINDOW_SIZE as u64 {
                return Err(StreamError::InvalidState {
                    expected: "valid window size".to_string(),
                    actual: format!("window overflow: {} + {}", current_window, increment),
                }.into());
            }
            
            self.stream_send_windows.insert(stream_id, new_window as u32);
        }
        
        Ok(())
    }
    
    /// Update receive window (typically after sending a window update frame).
    pub fn update_recv_window(
        &mut self,
        stream_id: u64,
        increment: u32,
    ) -> H3Result<()> {
        if stream_id == 0 {
            // Connection-level window update
            let new_window = self.connection_recv_window as u64 + increment as u64;
            if new_window > MAX_WINDOW_SIZE as u64 {
                return Err(StreamError::InvalidState {
                    expected: "valid window size".to_string(),
                    actual: format!("window overflow: {} + {}", self.connection_recv_window, increment),
                }.into());
            }
            self.connection_recv_window = new_window as u32;
        } else {
            // Stream-level window update
            let current_window = self.stream_recv_windows.get(&stream_id).copied()
                .unwrap_or(0);
            let new_window = current_window as u64 + increment as u64;
            
            if new_window > MAX_WINDOW_SIZE as u64 {
                return Err(StreamError::InvalidState {
                    expected: "valid window size".to_string(),
                    actual: format!("window overflow: {} + {}", current_window, increment),
                }.into());
            }
            
            self.stream_recv_windows.insert(stream_id, new_window as u32);
        }
        
        Ok(())
    }
    
    /// Get current send window for a stream.
    pub fn send_window(&self, stream_id: u64) -> u32 {
        if stream_id == 0 {
            self.connection_send_window
        } else {
            self.stream_send_windows.get(&stream_id).copied().unwrap_or(0)
        }
    }
    
    /// Get current receive window for a stream.
    pub fn recv_window(&self, stream_id: u64) -> u32 {
        if stream_id == 0 {
            self.connection_recv_window
        } else {
            self.stream_recv_windows.get(&stream_id).copied().unwrap_or(0)
        }
    }
    
    /// Get effective send window (minimum of connection and stream windows).
    pub fn effective_send_window(&self, stream_id: u64) -> u32 {
        if stream_id == 0 {
            self.connection_send_window
        } else {
            let stream_window = self.stream_send_windows.get(&stream_id).copied().unwrap_or(0);
            std::cmp::min(self.connection_send_window, stream_window)
        }
    }
    
    /// Check if a window update is needed.
    fn needs_window_update(&self, current_window: u32, initial_window: u32) -> bool {
        let threshold = (initial_window as f32 * WINDOW_UPDATE_THRESHOLD) as u32;
        current_window <= threshold
    }
    
    /// Calculate window update increment.
    fn calculate_window_update(&self, current_window: u32, initial_window: u32) -> u32 {
        initial_window - current_window
    }
    
    /// Get pending window updates.
    pub fn pending_window_updates(&self) -> &[WindowUpdate] {
        &self.pending_updates
    }
    
    /// Clear pending window updates.
    pub fn clear_pending_updates(&mut self) {
        self.pending_updates.clear();
    }
    
    /// Add a pending window update.
    pub fn add_pending_update(&mut self, stream_id: u64, increment: u32) {
        self.pending_updates.push(WindowUpdate {
            stream_id,
            increment,
        });
    }
    
    /// Get flow control statistics.
    pub fn get_stats(&self) -> FlowControlStats {
        FlowControlStats {
            connection_send_window: self.connection_send_window,
            connection_recv_window: self.connection_recv_window,
            total_bytes_sent: self.total_bytes_sent,
            total_bytes_received: self.total_bytes_received,
            active_streams: self.stream_send_windows.len(),
            pending_updates: self.pending_updates.len(),
        }
    }
    
    /// Reset flow control state.
    pub fn reset(&mut self) {
        self.connection_send_window = self.initial_connection_window;
        self.connection_recv_window = self.initial_connection_window;
        self.stream_send_windows.clear();
        self.stream_recv_windows.clear();
        self.total_bytes_sent = 0;
        self.total_bytes_received = 0;
        self.pending_updates.clear();
    }
    
    /// Update initial window sizes.
    pub fn update_initial_window_sizes(
        &mut self,
        connection_window: Option<u32>,
        stream_window: Option<u32>,
    ) -> H3Result<()> {
        if let Some(new_conn_window) = connection_window {
            if new_conn_window < MIN_WINDOW_SIZE || new_conn_window > MAX_WINDOW_SIZE {
                return Err(StreamError::InvalidState {
                    expected: format!("window size between {} and {}", MIN_WINDOW_SIZE, MAX_WINDOW_SIZE),
                    actual: format!("{}", new_conn_window),
                }.into());
            }
            
            let diff = new_conn_window as i64 - self.initial_connection_window as i64;
            self.initial_connection_window = new_conn_window;
            
            // Adjust current connection window
            let new_current = (self.connection_send_window as i64 + diff).max(0) as u32;
            self.connection_send_window = new_current.min(MAX_WINDOW_SIZE);
            
            let new_recv = (self.connection_recv_window as i64 + diff).max(0) as u32;
            self.connection_recv_window = new_recv.min(MAX_WINDOW_SIZE);
        }
        
        if let Some(new_stream_window) = stream_window {
            if new_stream_window < MIN_WINDOW_SIZE || new_stream_window > MAX_WINDOW_SIZE {
                return Err(StreamError::InvalidState {
                    expected: format!("window size between {} and {}", MIN_WINDOW_SIZE, MAX_WINDOW_SIZE),
                    actual: format!("{}", new_stream_window),
                }.into());
            }
            
            let diff = new_stream_window as i64 - self.initial_stream_window as i64;
            self.initial_stream_window = new_stream_window;
            
            // Adjust all existing stream windows
            for window in self.stream_send_windows.values_mut() {
                let new_window = (*window as i64 + diff).max(0) as u32;
                *window = new_window.min(MAX_WINDOW_SIZE);
            }
            
            for window in self.stream_recv_windows.values_mut() {
                let new_window = (*window as i64 + diff).max(0) as u32;
                *window = new_window.min(MAX_WINDOW_SIZE);
            }
        }
        
        Ok(())
    }
}

/// Flow control statistics.
#[derive(Debug, Clone)]
pub struct FlowControlStats {
    /// Connection send window
    pub connection_send_window: u32,
    /// Connection receive window
    pub connection_recv_window: u32,
    /// Total bytes sent
    pub total_bytes_sent: u64,
    /// Total bytes received
    pub total_bytes_received: u64,
    /// Number of active streams
    pub active_streams: usize,
    /// Number of pending window updates
    pub pending_updates: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_flow_controller_creation() {
        let controller = FlowController::new(65536, 32768);
        
        assert_eq!(controller.connection_send_window, 65536);
        assert_eq!(controller.connection_recv_window, 65536);
        assert_eq!(controller.initial_connection_window, 65536);
        assert_eq!(controller.initial_stream_window, 32768);
    }
    
    #[test]
    fn test_stream_initialization() {
        let mut controller = FlowController::new(65536, 32768);
        
        controller.initialize_stream(1).unwrap();
        
        assert_eq!(controller.send_window(1), 32768);
        assert_eq!(controller.recv_window(1), 32768);
    }
    
    #[test]
    fn test_send_window_reservation() {
        let mut controller = FlowController::new(65536, 32768);
        controller.initialize_stream(1).unwrap();
        
        let events = controller.reserve_send_window(1, 1000).unwrap();
        assert!(events.is_empty());
        
        assert_eq!(controller.send_window(1), 31768);
        assert_eq!(controller.connection_send_window, 64536);
    }
    
    #[test]
    fn test_flow_control_blocking() {
        let mut controller = FlowController::new(1000, 500);
        controller.initialize_stream(1).unwrap();
        
        // Try to send more than stream window allows
        let events = controller.reserve_send_window(1, 600).unwrap();
        assert_eq!(events.len(), 1);
        
        match &events[0] {
            FlowControlEvent::FlowControlBlocked { stream_id, available_window, requested_bytes } => {
                assert_eq!(*stream_id, 1);
                assert_eq!(*available_window, 500);
                assert_eq!(*requested_bytes, 600);
            }
            _ => panic!("Expected FlowControlBlocked event"),
        }
    }
    
    #[test]
    fn test_received_data_processing() {
        let mut controller = FlowController::new(65536, 32768);
        controller.initialize_stream(1).unwrap();
        
        let events = controller.process_received_data(1, 1000).unwrap();
        
        assert_eq!(controller.recv_window(1), 31768);
        assert_eq!(controller.connection_recv_window, 64536);
        
        // Should not generate window update events yet (above threshold)
        assert!(events.is_empty());
    }
    
    #[test]
    fn test_window_update_needed() {
        let mut controller = FlowController::new(1000, 1000);
        controller.initialize_stream(1).unwrap();
        
        // Consume more than 50% of the window
        let events = controller.process_received_data(1, 600).unwrap();
        
        // Should generate window update event
        assert!(!events.is_empty());
        
        // Find the window update event for stream 1
        let mut found = false;
        for event in &events {
            if let FlowControlEvent::WindowUpdateNeeded { stream_id, increment } = event {
                if *stream_id == 1 {
                    found = true;
                    assert_eq!(*increment, 600); // Restore to initial window
                }
            }
        }
        assert!(found, "Expected WindowUpdateNeeded event for stream 1");
    }
    
    #[test]
    fn test_send_window_update() {
        let mut controller = FlowController::new(65536, 32768);
        controller.initialize_stream(1).unwrap();
        
        // Reserve some window
        controller.reserve_send_window(1, 1000).unwrap();
        assert_eq!(controller.send_window(1), 31768);
        
        // Update window
        controller.update_send_window(1, 500).unwrap();
        assert_eq!(controller.send_window(1), 32268);
    }
    
    #[test]
    fn test_effective_send_window() {
        let mut controller = FlowController::new(1000, 2000);
        controller.initialize_stream(1).unwrap();
        
        // Connection window is smaller
        assert_eq!(controller.effective_send_window(1), 1000);
        
        // Reserve some connection window
        controller.reserve_send_window(1, 500).unwrap();
        
        // Now stream window is smaller
        assert_eq!(controller.effective_send_window(1), 500);
    }
    
    #[test]
    fn test_window_overflow_protection() {
        let mut controller = FlowController::new(65536, 32768);
        controller.initialize_stream(1).unwrap();
        
        // Try to update window beyond maximum
        let result = controller.update_send_window(1, MAX_WINDOW_SIZE);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_flow_control_stats() {
        let mut controller = FlowController::new(65536, 32768);
        controller.initialize_stream(1).unwrap();
        controller.initialize_stream(2).unwrap();
        
        controller.reserve_send_window(1, 1000).unwrap();
        controller.process_received_data(1, 500).unwrap();
        
        let stats = controller.get_stats();
        assert_eq!(stats.active_streams, 2);
        assert_eq!(stats.total_bytes_sent, 1000);
        assert_eq!(stats.total_bytes_received, 500);
    }
    
    #[test]
    fn test_initial_window_size_update() {
        let mut controller = FlowController::new(65536, 32768);
        controller.initialize_stream(1).unwrap();
        
        // Update initial stream window size
        controller.update_initial_window_sizes(None, Some(16384)).unwrap();
        
        assert_eq!(controller.initial_stream_window, 16384);
        // Existing stream window should be adjusted
        assert_eq!(controller.send_window(1), 16384);
    }
}