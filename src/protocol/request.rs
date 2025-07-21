//! HTTP/3 request types and builders.

use crate::config::Priority;
use crate::protocol::Body;
use http::{HeaderMap, Method, Uri};

/// An HTTP/3 request.
#[derive(Debug, Clone)]
pub struct H3Request {
    /// HTTP method
    pub method: Method,
    
    /// Request URI
    pub uri: Uri,
    
    /// Request headers
    pub headers: HeaderMap,
    
    /// Request body
    pub body: Option<Body>,
    
    /// Stream priority
    pub priority: Priority,
}

impl H3Request {
    /// Create a new request with the given method and URI.
    pub fn new(method: Method, uri: Uri) -> Self {
        Self {
            method,
            uri,
            headers: HeaderMap::new(),
            body: None,
            priority: Priority::default(),
        }
    }
    
    /// Create a GET request.
    pub fn get<T>(uri: T) -> H3RequestBuilder
    where
        T: TryInto<Uri>,
        T::Error: Into<http::Error>,
    {
        H3RequestBuilder::new(Method::GET, uri)
    }
    
    /// Create a POST request.
    pub fn post<T>(uri: T) -> H3RequestBuilder
    where
        T: TryInto<Uri>,
        T::Error: Into<http::Error>,
    {
        H3RequestBuilder::new(Method::POST, uri)
    }
    
    /// Create a PUT request.
    pub fn put<T>(uri: T) -> H3RequestBuilder
    where
        T: TryInto<Uri>,
        T::Error: Into<http::Error>,
    {
        H3RequestBuilder::new(Method::PUT, uri)
    }
    
    /// Create a DELETE request.
    pub fn delete<T>(uri: T) -> H3RequestBuilder
    where
        T: TryInto<Uri>,
        T::Error: Into<http::Error>,
    {
        H3RequestBuilder::new(Method::DELETE, uri)
    }
    
    /// Create a PATCH request.
    pub fn patch<T>(uri: T) -> H3RequestBuilder
    where
        T: TryInto<Uri>,
        T::Error: Into<http::Error>,
    {
        H3RequestBuilder::new(Method::PATCH, uri)
    }
    
    /// Get the content length if available.
    pub fn content_length(&self) -> Option<u64> {
        self.headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
    }
    
    /// Check if the request has a body.
    pub fn has_body(&self) -> bool {
        self.body.is_some()
    }
    
    /// Get the request authority (host).
    pub fn authority(&self) -> Option<&str> {
        self.uri.authority().map(|a| a.as_str())
    }
    
    /// Get the request path.
    pub fn path(&self) -> &str {
        self.uri.path()
    }
    
    /// Get the request query string.
    pub fn query(&self) -> Option<&str> {
        self.uri.query()
    }
}

/// Builder for constructing H3 requests.
#[derive(Debug)]
pub struct H3RequestBuilder {
    method: Method,
    uri: Result<Uri, http::Error>,
    headers: HeaderMap,
    body: Option<Body>,
    priority: Priority,
}

impl H3RequestBuilder {
    /// Create a new request builder.
    pub fn new<T>(method: Method, uri: T) -> Self
    where
        T: TryInto<Uri>,
        T::Error: Into<http::Error>,
    {
        Self {
            method,
            uri: uri.try_into().map_err(Into::into),
            headers: HeaderMap::new(),
            body: None,
            priority: Priority::default(),
        }
    }
    
    /// Add a header to the request.
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        K: TryInto<http::HeaderName>,
        K::Error: Into<http::Error>,
        V: TryInto<http::HeaderValue>,
        V::Error: Into<http::Error>,
    {
        if let (Ok(key), Ok(value)) = (key.try_into(), value.try_into()) {
            self.headers.insert(key, value);
        }
        self
    }
    
    /// Set the request body.
    pub fn body<B>(mut self, body: B) -> Self
    where
        B: Into<Body>,
    {
        self.body = Some(body.into());
        self
    }
    
    /// Set the request priority.
    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }
    
    /// Set the content type header.
    pub fn content_type<V>(self, content_type: V) -> Self
    where
        V: TryInto<http::HeaderValue>,
        V::Error: Into<http::Error>,
    {
        self.header(http::header::CONTENT_TYPE, content_type)
    }
    
    /// Set the user agent header.
    pub fn user_agent<V>(self, user_agent: V) -> Self
    where
        V: TryInto<http::HeaderValue>,
        V::Error: Into<http::Error>,
    {
        self.header(http::header::USER_AGENT, user_agent)
    }
    
    /// Set the authorization header.
    pub fn authorization<V>(self, auth: V) -> Self
    where
        V: TryInto<http::HeaderValue>,
        V::Error: Into<http::Error>,
    {
        self.header(http::header::AUTHORIZATION, auth)
    }
    
    /// Build the request.
    pub fn build(self) -> Result<H3Request, http::Error> {
        Ok(H3Request {
            method: self.method,
            uri: self.uri?,
            headers: self.headers,
            body: self.body,
            priority: self.priority,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_request_builder() {
        let request = H3Request::get("https://example.com/api/data")
            .header("accept", "application/json")
            .user_agent("wasm-h3/1.0")
            .build()
            .unwrap();
        
        assert_eq!(request.method, Method::GET);
        assert_eq!(request.uri.path(), "/api/data");
        assert_eq!(request.headers.get("accept").unwrap(), "application/json");
        assert_eq!(request.headers.get("user-agent").unwrap(), "wasm-h3/1.0");
    }
    
    #[test]
    fn test_post_with_body() {
        let body = Body::from("test data");
        let request = H3Request::post("https://example.com/api/submit")
            .content_type("text/plain")
            .body(body)
            .build()
            .unwrap();
        
        assert_eq!(request.method, Method::POST);
        assert!(request.has_body());
        assert_eq!(request.headers.get("content-type").unwrap(), "text/plain");
    }
    
    #[test]
    fn test_request_properties() {
        let request = H3Request::get("https://example.com:8080/path?query=value")
            .build()
            .unwrap();
        
        assert_eq!(request.authority(), Some("example.com:8080"));
        assert_eq!(request.path(), "/path");
        assert_eq!(request.query(), Some("query=value"));
    }
}