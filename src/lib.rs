//! # WASM H3 - HTTP/3 over QUIC for WebAssembly
//!
//! A WebAssembly-compatible HTTP/3 library that provides both client and server
//! implementations built on top of QUIC transport protocol.
//!
//! ## Features
//!
//! - **Client Support**: Send HTTP/3 requests with full multiplexing support
//! - **Server Support**: Handle incoming HTTP/3 requests with concurrent connections
//! - **WASM Compatible**: Designed to work within WASI runtime constraints
//! - **Async/Await**: Built on tokio for efficient async operations
//! - **Zero-Copy**: Optimized parsing and buffer management
//!
//! ## Example Usage
//!
//! ```rust,no_run
//! use wasm_h3::{H3Client, ClientConfig, H3Request};
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = ClientConfig::default();
//!     let mut client = H3Client::connect("https://example.com", config).await?;
//!     
//!     let request = H3Request::get("https://example.com/api/data").build()?;
//!     let response = client.send_request(request).await?;
//!     
//!     println!("Status: {}", response.status);
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod server;
pub mod protocol;
pub mod transport;
pub mod network;
pub mod error;
pub mod config;

// Re-export main types for convenience
pub use client::H3Client;
pub use server::H3Server;
pub use error::{H3Error, H3Result};
pub use config::{ClientConfig, ServerConfig};

// Re-export protocol types
pub use protocol::{H3Request, H3Response, H3Frame, Body};

// Re-export common HTTP types
pub use http::{Method, StatusCode, HeaderMap, Uri};