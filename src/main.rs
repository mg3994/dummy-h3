use wasm_h3::{H3Client, H3Request, ClientConfig};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();
    
    println!("WASM H3 Library Demo");
    
    // Create client configuration
    let config = ClientConfig::builder()
        .max_concurrent_streams(50)
        .connection_timeout(std::time::Duration::from_secs(5))
        .build();
    
    // Connect to server (placeholder implementation)
    let mut client = H3Client::connect("https://example.com", config).await?;
    
    // Create and send a request
    let request = H3Request::get("https://example.com/api/data")
        .header("accept", "application/json")
        .user_agent("wasm-h3/1.0")
        .build()?;
    
    println!("Sending request: {} {}", request.method, request.uri);
    
    let response = client.send_request(request).await?;
    
    println!("Received response: {}", response.status);
    println!("Response headers: {:?}", response.headers);
    
    // Close the client
    client.close().await?;
    
    println!("Demo completed successfully!");
    
    Ok(())
}