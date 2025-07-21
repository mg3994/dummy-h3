//! Configuration types for H3 client and server.

use std::time::Duration;

// Re-export Priority from protocol module
pub use crate::protocol::frame::Priority;

/// Configuration for H3 client connections.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Maximum number of concurrent streams per connection
    pub max_concurrent_streams: u64,
    
    /// Initial flow control window size
    pub initial_window_size: u32,
    
    /// Connection establishment timeout
    pub connection_timeout: Duration,
    
    /// Connection idle timeout
    pub idle_timeout: Duration,
    
    /// TLS configuration
    pub tls_config: TlsClientConfig,
    
    /// ALPN protocols to negotiate
    pub alpn_protocols: Vec<String>,
    
    /// Enable connection migration
    pub enable_migration: bool,
    
    /// Keep-alive interval
    pub keep_alive_interval: Option<Duration>,
    
    /// Maximum frame size
    pub max_frame_size: u32,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_concurrent_streams: 100,
            initial_window_size: 65536,
            connection_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(30),
            tls_config: TlsClientConfig::default(),
            alpn_protocols: vec!["h3".to_string()],
            enable_migration: true,
            keep_alive_interval: Some(Duration::from_secs(15)),
            max_frame_size: 16384,
        }
    }
}

impl ClientConfig {
    /// Create a new client configuration builder
    pub fn builder() -> ClientConfigBuilder {
        ClientConfigBuilder::default()
    }
}

/// Configuration for H3 server.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Maximum number of concurrent streams per connection
    pub max_concurrent_streams: u64,
    
    /// Initial flow control window size
    pub initial_window_size: u32,
    
    /// Maximum frame size
    pub max_frame_size: u32,
    
    /// Connection timeout
    pub connection_timeout: Duration,
    
    /// TLS configuration
    pub tls_config: TlsServerConfig,
    
    /// Certificate chain
    pub certificate_chain: Vec<Certificate>,
    
    /// Private key
    pub private_key: PrivateKey,
    
    /// Enable server push
    pub enable_push: bool,
    
    /// Maximum number of concurrent connections
    pub max_connections: Option<u64>,
    
    /// Connection idle timeout
    pub idle_timeout: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_streams: 100,
            initial_window_size: 65536,
            max_frame_size: 16384,
            connection_timeout: Duration::from_secs(10),
            tls_config: TlsServerConfig::default(),
            certificate_chain: Vec::new(),
            private_key: PrivateKey::default(),
            enable_push: false,
            max_connections: Some(1000),
            idle_timeout: Duration::from_secs(60),
        }
    }
}

impl ServerConfig {
    /// Create a new server configuration builder
    pub fn builder() -> ServerConfigBuilder {
        ServerConfigBuilder::default()
    }
}

/// TLS configuration for client connections.
#[derive(Debug, Clone)]
pub struct TlsClientConfig {
    /// Verify server certificates
    pub verify_certificates: bool,
    
    /// Custom certificate authorities
    pub custom_ca_certs: Vec<Certificate>,
    
    /// Client certificate for mutual TLS
    pub client_cert: Option<Certificate>,
    
    /// Client private key for mutual TLS
    pub client_key: Option<PrivateKey>,
    
    /// Server name for SNI
    pub server_name: Option<String>,
}

impl Default for TlsClientConfig {
    fn default() -> Self {
        Self {
            verify_certificates: true,
            custom_ca_certs: Vec::new(),
            client_cert: None,
            client_key: None,
            server_name: None,
        }
    }
}

/// TLS configuration for server.
#[derive(Debug, Clone)]
pub struct TlsServerConfig {
    /// Require client certificates
    pub require_client_certs: bool,
    
    /// Custom certificate authorities for client verification
    pub client_ca_certs: Vec<Certificate>,
    
    /// Supported cipher suites
    pub cipher_suites: Vec<CipherSuite>,
    
    /// Supported signature algorithms
    pub signature_algorithms: Vec<SignatureAlgorithm>,
}

impl Default for TlsServerConfig {
    fn default() -> Self {
        Self {
            require_client_certs: false,
            client_ca_certs: Vec::new(),
            cipher_suites: Vec::new(),
            signature_algorithms: Vec::new(),
        }
    }
}

/// Stream configuration.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Initial flow control window size
    pub initial_window_size: u32,
    
    /// Maximum frame size for this stream
    pub max_frame_size: u32,
    
    /// Stream priority
    pub priority: crate::protocol::stream::StreamPriority,
    
    /// Enable flow control
    pub enable_flow_control: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            initial_window_size: 65536,
            max_frame_size: 16384,
            priority: crate::protocol::stream::StreamPriority::default(),
            enable_flow_control: true,
        }
    }
}

/// Certificate representation.
#[derive(Debug, Clone)]
pub struct Certificate {
    /// DER-encoded certificate data
    pub der: Vec<u8>,
}

impl Default for Certificate {
    fn default() -> Self {
        Self { der: Vec::new() }
    }
}

/// Private key representation.
#[derive(Debug, Clone)]
pub struct PrivateKey {
    /// DER-encoded private key data
    pub der: Vec<u8>,
}

impl Default for PrivateKey {
    fn default() -> Self {
        Self { der: Vec::new() }
    }
}



/// TLS cipher suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    Aes128GcmSha256,
    Aes256GcmSha384,
    ChaCha20Poly1305Sha256,
}

/// TLS signature algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    RsaPkcs1Sha256,
    RsaPkcs1Sha384,
    RsaPkcs1Sha512,
    EcdsaSecp256r1Sha256,
    EcdsaSecp384r1Sha384,
    RsaPssRsaeSha256,
    RsaPssRsaeSha384,
    RsaPssRsaeSha512,
}

/// Builder for client configuration.
#[derive(Debug, Default)]
pub struct ClientConfigBuilder {
    config: ClientConfig,
}

impl ClientConfigBuilder {
    /// Set maximum concurrent streams
    pub fn max_concurrent_streams(mut self, max: u64) -> Self {
        self.config.max_concurrent_streams = max;
        self
    }
    
    /// Set initial window size
    pub fn initial_window_size(mut self, size: u32) -> Self {
        self.config.initial_window_size = size;
        self
    }
    
    /// Set connection timeout
    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.config.connection_timeout = timeout;
        self
    }
    
    /// Set idle timeout
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.config.idle_timeout = timeout;
        self
    }
    
    /// Enable or disable connection migration
    pub fn enable_migration(mut self, enable: bool) -> Self {
        self.config.enable_migration = enable;
        self
    }
    
    /// Set keep-alive interval
    pub fn keep_alive_interval(mut self, interval: Option<Duration>) -> Self {
        self.config.keep_alive_interval = interval;
        self
    }
    
    /// Build the configuration
    pub fn build(self) -> ClientConfig {
        self.config
    }
}

/// Builder for server configuration.
#[derive(Debug, Default)]
pub struct ServerConfigBuilder {
    config: ServerConfig,
}

impl ServerConfigBuilder {
    /// Set maximum concurrent streams
    pub fn max_concurrent_streams(mut self, max: u64) -> Self {
        self.config.max_concurrent_streams = max;
        self
    }
    
    /// Set initial window size
    pub fn initial_window_size(mut self, size: u32) -> Self {
        self.config.initial_window_size = size;
        self
    }
    
    /// Set maximum frame size
    pub fn max_frame_size(mut self, size: u32) -> Self {
        self.config.max_frame_size = size;
        self
    }
    
    /// Set certificate chain
    pub fn certificate_chain(mut self, chain: Vec<Certificate>) -> Self {
        self.config.certificate_chain = chain;
        self
    }
    
    /// Set private key
    pub fn private_key(mut self, key: PrivateKey) -> Self {
        self.config.private_key = key;
        self
    }
    
    /// Enable or disable server push
    pub fn enable_push(mut self, enable: bool) -> Self {
        self.config.enable_push = enable;
        self
    }
    
    /// Set maximum connections
    pub fn max_connections(mut self, max: Option<u64>) -> Self {
        self.config.max_connections = max;
        self
    }
    
    /// Build the configuration
    pub fn build(self) -> ServerConfig {
        self.config
    }
}