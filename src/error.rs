//! Error types and handling for the H3 library.

use std::fmt;

/// Result type alias for H3 operations.
pub type H3Result<T> = Result<T, H3Error>;

/// Network-related errors
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    /// I/O error
    #[error("I/O error: {0}")]
    IoError(std::io::Error),
    
    /// Address resolution failed
    #[error("Failed to resolve address")]
    AddressResolutionFailed,
    
    /// Connection refused
    #[error("Connection refused")]
    ConnectionRefused,
    
    /// Connection reset
    #[error("Connection reset")]
    ConnectionReset,
    
    /// Connection timed out
    #[error("Connection timed out")]
    ConnectionTimeout,
    
    /// Network unreachable
    #[error("Network unreachable")]
    NetworkUnreachable,
    
    /// Operation not supported
    #[error("Operation not supported: {0}")]
    OperationNotSupported(String),
    
    /// Permission denied
    #[error("Permission denied")]
    PermissionDenied,
    
    /// Address already in use
    #[error("Address already in use")]
    AddressInUse,
    
    /// Other network error
    #[error("Network error: {0}")]
    Other(String),
}

/// Main error type for H3 operations.
#[derive(Debug, thiserror::Error)]
pub enum H3Error {
    /// Connection-related errors
    #[error("Connection error: {0}")]
    Connection(#[from] ConnectionError),
    
    /// Protocol-level errors
    #[error("Protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    
    /// Stream-related errors
    #[error("Stream error: {0}")]
    Stream(#[from] StreamError),
    
    /// Frame parsing errors
    #[error("Parse error: {0}")]
    Parse(#[from] ParseError),
    
    /// TLS/Crypto errors
    #[error("TLS error: {0}")]
    Tls(#[from] TlsError),
    
    /// Network-related errors
    #[error("Network error: {0}")]
    Network(#[from] NetworkError),
    
    /// Timeout errors
    #[error("Operation timed out")]
    Timeout,
    
    /// Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),
    
    /// I/O errors
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    
    /// Generic errors
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Connection-specific errors
#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("Failed to establish connection")]
    EstablishmentFailed,
    
    #[error("Connection was closed unexpectedly")]
    UnexpectedClose,
    
    #[error("Connection handshake failed: {0}")]
    HandshakeFailed(String),
    
    #[error("Connection migration failed")]
    MigrationFailed,
    
    #[error("Connection idle timeout")]
    IdleTimeout,
    
    #[error("Invalid connection state: {0}")]
    InvalidState(String),
}

/// Protocol-level errors
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("Invalid HTTP/3 frame type: {0}")]
    InvalidFrameType(u64),
    
    #[error("Protocol violation: {0}")]
    ProtocolViolation(String),
    
    #[error("Unsupported HTTP version")]
    UnsupportedVersion,
    
    #[error("Invalid ALPN protocol")]
    InvalidAlpn,
    
    #[error("Flow control violation")]
    FlowControlViolation,
    
    #[error("Settings error: {0}")]
    Settings(String),
}

/// Stream-related errors
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("Stream not found: {0}")]
    NotFound(u64),
    
    #[error("Stream already closed: {0}")]
    AlreadyClosed(u64),
    
    #[error("Invalid stream state: expected {expected}, got {actual}")]
    InvalidState { expected: String, actual: String },
    
    #[error("Stream limit exceeded")]
    LimitExceeded,
    
    #[error("Stream reset by peer: {0}")]
    Reset(u64),
    
    #[error("Stream blocked")]
    Blocked,
}

/// Frame parsing errors
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Incomplete frame data")]
    Incomplete,
    
    #[error("Invalid frame format: {0}")]
    InvalidFormat(String),
    
    #[error("Frame too large: {size} bytes (max: {max})")]
    FrameTooLarge { size: usize, max: usize },
    
    #[error("Invalid header compression")]
    HeaderCompression,
    
    #[error("Invalid varint encoding")]
    InvalidVarint,
    
    #[error("Buffer overflow")]
    BufferOverflow,
}

/// TLS and cryptographic errors
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("Certificate validation failed: {0}")]
    CertificateValidation(String),
    
    #[error("Handshake failed: {0}")]
    HandshakeFailed(String),
    
    #[error("Encryption failed")]
    EncryptionFailed,
    
    #[error("Decryption failed")]
    DecryptionFailed,
    
    #[error("Key derivation failed")]
    KeyDerivationFailed,
    
    #[error("Invalid certificate")]
    InvalidCertificate,
}

/// Body-related errors
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error("Body read error: {0}")]
    Read(String),
    
    #[error("Body write error: {0}")]
    Write(String),
    
    #[error("Body too large")]
    TooLarge,
    
    #[error("Body stream ended unexpectedly")]
    UnexpectedEnd,
}

impl H3Error {
    /// Create a new configuration error
    pub fn config<T: fmt::Display>(msg: T) -> Self {
        H3Error::Config(msg.to_string())
    }
    
    /// Create a new internal error
    pub fn internal<T: fmt::Display>(msg: T) -> Self {
        H3Error::Internal(msg.to_string())
    }
    
    /// Check if this error is recoverable
    pub fn is_recoverable(&self) -> bool {
        match self {
            H3Error::Connection(ConnectionError::IdleTimeout) => true,
            H3Error::Stream(StreamError::Blocked) => true,
            H3Error::Timeout => true,
            H3Error::Io(_) => true,
            _ => false,
        }
    }
    
    /// Check if this error should trigger a connection close
    pub fn should_close_connection(&self) -> bool {
        match self {
            H3Error::Connection(_) => true,
            H3Error::Protocol(_) => true,
            H3Error::Tls(_) => true,
            _ => false,
        }
    }
}