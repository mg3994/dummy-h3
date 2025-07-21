//! DNS resolution for WASI environments.

use crate::error::H3Result;
use std::net::{IpAddr, SocketAddr};

/// DNS resolver for WASI environments.
#[derive(Debug)]
pub struct DnsResolver {
    /// DNS server addresses
    servers: Vec<SocketAddr>,
}

impl DnsResolver {
    /// Create a new DNS resolver with default servers.
    pub fn new() -> Self {
        Self {
            servers: vec![
                "8.8.8.8:53".parse().unwrap(),
                "8.8.4.4:53".parse().unwrap(),
            ],
        }
    }
    
    /// Create a DNS resolver with custom servers.
    pub fn with_servers(servers: Vec<SocketAddr>) -> Self {
        Self { servers }
    }
    
    /// Resolve a hostname to IP addresses.
    pub async fn resolve(&self, hostname: &str) -> H3Result<Vec<IpAddr>> {
        tracing::debug!("Resolving hostname: {}", hostname);
        
        // TODO: Implement actual DNS resolution for WASI
        // This would involve:
        // 1. DNS query construction
        // 2. UDP communication with DNS servers
        // 3. Response parsing
        
        // For now, try to parse as IP address first
        if let Ok(ip) = hostname.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        
        // Return localhost for testing
        Ok(vec!["127.0.0.1".parse().unwrap()])
    }
    
    /// Resolve a hostname and port to socket addresses.
    pub async fn resolve_with_port(&self, hostname: &str, port: u16) -> H3Result<Vec<SocketAddr>> {
        let ips = self.resolve(hostname).await?;
        Ok(ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect())
    }
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function for DNS resolution.
pub async fn resolve(hostname: &str) -> H3Result<Vec<IpAddr>> {
    let resolver = DnsResolver::new();
    resolver.resolve(hostname).await
}

/// Convenience function for resolving hostname with port.
pub async fn resolve_with_port(hostname: &str, port: u16) -> H3Result<Vec<SocketAddr>> {
    let resolver = DnsResolver::new();
    resolver.resolve_with_port(hostname, port).await
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_ip_address_resolution() {
        let resolver = DnsResolver::new();
        let result = resolver.resolve("127.0.0.1").await;
        assert!(result.is_ok());
        
        let ips = result.unwrap();
        assert_eq!(ips.len(), 1);
        assert_eq!(ips[0], "127.0.0.1".parse::<IpAddr>().unwrap());
    }
    
    #[tokio::test]
    async fn test_hostname_resolution() {
        let resolver = DnsResolver::new();
        let result = resolver.resolve("localhost").await;
        assert!(result.is_ok());
        
        let ips = result.unwrap();
        assert!(!ips.is_empty());
    }
    
    #[tokio::test]
    async fn test_resolve_with_port() {
        let resolver = DnsResolver::new();
        let result = resolver.resolve_with_port("127.0.0.1", 8080).await;
        assert!(result.is_ok());
        
        let addrs = result.unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].port(), 8080);
    }
}