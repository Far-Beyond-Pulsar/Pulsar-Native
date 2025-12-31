//! Dead simple TCP server for testing port forwarding
//! No tokio, no async, no axum - just raw stdlib sockets like Python

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

pub fn run() -> anyhow::Result<()> {
    let bind_addr = "0.0.0.0:8080";
    let listener = TcpListener::bind(bind_addr)?;
    tracing::debug!("✅ Simple test server listening on {}", bind_addr);
    tracing::debug!("Try: curl http://YOUR_PUBLIC_IP:8080/");
    
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(|| handle_client(stream));
            }
            Err(e) => {
                tracing::error!("Connection failed: {}", e);
            }
        }
    }
    
    Ok(())
}

fn handle_client(mut stream: TcpStream) {
    let peer_addr = stream.peer_addr().unwrap();
    tracing::debug!("✅ Connection from: {}", peer_addr);
    
    let mut buffer = [0; 1024];
    match stream.read(&mut buffer) {
        Ok(size) => {
            tracing::debug!("📨 Received {} bytes from {}", size, peer_addr);
            
            // Send HTTP response
            let body = "Simple test server works!\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/plain\r\n\
                 Content-Length: {}\r\n\
                 \r\n\
                 {}",
                body.len(),
                body
            );
            
            if let Err(e) = stream.write_all(response.as_bytes()) {
                tracing::error!("Failed to send response: {}", e);
            } else {
                tracing::debug!("✅ Sent response to {}", peer_addr);
            }
        }
        Err(e) => {
            tracing::error!("Failed to read from {}: {}", peer_addr, e);
        }
    }
}
