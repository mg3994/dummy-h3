//! HTTP/3 response types and body handling.

use crate::error::BodyError;
use bytes::Bytes;
use futures::Stream;
use http::{HeaderMap, StatusCode};
use std::pin::Pin;
use std::task::{Context, Poll};

/// An HTTP/3 response.
#[derive(Debug)]
pub struct H3Response {
    /// Response status code
    pub status: StatusCode,
    
    /// Response headers
    pub headers: HeaderMap,
    
    /// Response body
    pub body: Body,
    
    /// Response trailers (sent after body)
    pub trailers: Option<HeaderMap>,
}

impl H3Response {
    /// Create a new response with the given status code.
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: Body::Empty,
            trailers: None,
        }
    }
    
    /// Create a 200 OK response.
    pub fn ok() -> Self {
        Self::new(StatusCode::OK)
    }
    
    /// Create a 404 Not Found response.
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND)
    }
    
    /// Create a 500 Internal Server Error response.
    pub fn internal_server_error() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR)
    }
    
    /// Get the content length if available.
    pub fn content_length(&self) -> Option<u64> {
        self.headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
    }
    
    /// Check if the response is successful (2xx status code).
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }
    
    /// Check if the response is a client error (4xx status code).
    pub fn is_client_error(&self) -> bool {
        self.status.is_client_error()
    }
    
    /// Check if the response is a server error (5xx status code).
    pub fn is_server_error(&self) -> bool {
        self.status.is_server_error()
    }
    
    /// Get the content type header.
    pub fn content_type(&self) -> Option<&str> {
        self.headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
    }
    
    /// Set the response body.
    pub fn with_body<B>(mut self, body: B) -> Self
    where
        B: Into<Body>,
    {
        self.body = body.into();
        self
    }
    
    /// Set a header on the response.
    pub fn with_header<K, V>(mut self, key: K, value: V) -> Self
    where
        K: TryInto<http::HeaderName>,
        V: TryInto<http::HeaderValue>,
    {
        if let (Ok(key), Ok(value)) = (key.try_into(), value.try_into()) {
            self.headers.insert(key, value);
        }
        self
    }
}

/// HTTP/3 request/response body.
pub enum Body {
    /// Empty body
    Empty,
    
    /// Body with known bytes
    Bytes(Bytes),
    
    /// Streaming body
    Stream(Box<dyn Stream<Item = Result<Bytes, BodyError>> + Send + Unpin>),
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Body::Empty => write!(f, "Body::Empty"),
            Body::Bytes(bytes) => write!(f, "Body::Bytes({} bytes)", bytes.len()),
            Body::Stream(_) => write!(f, "Body::Stream(...)"),
        }
    }
}

impl Clone for Body {
    fn clone(&self) -> Self {
        match self {
            Body::Empty => Body::Empty,
            Body::Bytes(bytes) => Body::Bytes(bytes.clone()),
            Body::Stream(_) => {
                // Streams can't be cloned, so we create an empty body
                // In a real implementation, you might want to handle this differently
                Body::Empty
            }
        }
    }
}

impl Body {
    /// Create an empty body.
    pub fn empty() -> Self {
        Body::Empty
    }
    
    /// Create a body from bytes.
    pub fn from_bytes(bytes: Bytes) -> Self {
        Body::Bytes(bytes)
    }
    
    /// Create a body from a stream.
    pub fn from_stream<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes, BodyError>> + Send + Unpin + 'static,
    {
        Body::Stream(Box::new(stream))
    }
    
    /// Check if the body is empty.
    pub fn is_empty(&self) -> bool {
        matches!(self, Body::Empty)
    }
    
    /// Get the body size if known.
    pub fn size_hint(&self) -> Option<usize> {
        match self {
            Body::Empty => Some(0),
            Body::Bytes(bytes) => Some(bytes.len()),
            Body::Stream(_) => None,
        }
    }
    
    /// Convert the body to bytes if possible.
    pub async fn to_bytes(self) -> Result<Bytes, BodyError> {
        match self {
            Body::Empty => Ok(Bytes::new()),
            Body::Bytes(bytes) => Ok(bytes),
            Body::Stream(mut stream) => {
                let mut buf = Vec::new();
                while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
                    let chunk = chunk?;
                    buf.extend_from_slice(&chunk);
                }
                Ok(Bytes::from(buf))
            }
        }
    }
}

impl From<()> for Body {
    fn from(_: ()) -> Self {
        Body::Empty
    }
}

impl From<Vec<u8>> for Body {
    fn from(vec: Vec<u8>) -> Self {
        Body::Bytes(Bytes::from(vec))
    }
}

impl From<&[u8]> for Body {
    fn from(slice: &[u8]) -> Self {
        Body::Bytes(Bytes::copy_from_slice(slice))
    }
}

impl From<String> for Body {
    fn from(string: String) -> Self {
        Body::Bytes(Bytes::from(string))
    }
}

impl From<&str> for Body {
    fn from(s: &str) -> Self {
        Body::Bytes(Bytes::from(s.to_owned()))
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Self {
        Body::Bytes(bytes)
    }
}

/// A stream of body chunks.
pub struct BodyStream {
    inner: Box<dyn Stream<Item = Result<Bytes, BodyError>> + Send + Unpin>,
}

impl BodyStream {
    /// Create a new body stream.
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes, BodyError>> + Send + Unpin + 'static,
    {
        Self {
            inner: Box::new(stream),
        }
    }
}

impl Stream for BodyStream {
    type Item = Result<Bytes, BodyError>;
    
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_response_creation() {
        let response = H3Response::ok()
            .with_header("content-type", "application/json")
            .with_body("{}");
        
        assert_eq!(response.status, StatusCode::OK);
        assert!(response.is_success());
        assert_eq!(response.content_type(), Some("application/json"));
    }
    
    #[test]
    fn test_body_from_string() {
        let body = Body::from("test data");
        assert!(!body.is_empty());
        assert_eq!(body.size_hint(), Some(9));
    }
    
    #[test]
    fn test_empty_body() {
        let body = Body::empty();
        assert!(body.is_empty());
        assert_eq!(body.size_hint(), Some(0));
    }
    
    #[tokio::test]
    async fn test_body_to_bytes() {
        let body = Body::from("test data");
        let bytes = body.to_bytes().await.unwrap();
        assert_eq!(bytes, "test data");
    }
}