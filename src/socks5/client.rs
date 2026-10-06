// src/socks5/client.rs
//! SOCKS5 Client Implementation
//! =============================
//!
//! Connect through SOCKS5 proxy server

use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, anyhow};

use super::protocol::*;
use super::Socks5Error;

/// SOCKS5 client
pub struct Socks5Client {
    proxy_addr: SocketAddr,
    username: Option<String>,
    password: Option<String>,
}

impl Socks5Client {
    /// Create new SOCKS5 client
    pub fn new(proxy_addr: SocketAddr) -> Self {
        Self {
            proxy_addr,
            username: None,
            password: None,
        }
    }

    /// Set authentication credentials
    pub fn with_auth(mut self, username: String, password: String) -> Self {
        self.username = Some(username);
        self.password = Some(password);
        self
    }

    /// Connect through SOCKS5 proxy
    pub async fn connect(&self, target_addr: SocketAddr) -> Result<TcpStream> {
        // Connect to proxy
        let mut stream = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            TcpStream::connect(self.proxy_addr)
        ).await
        .map_err(|_| anyhow!("Connection timeout to SOCKS5 proxy"))?
        .map_err(|e| anyhow!("Failed to connect to SOCKS5 proxy: {}", e))?;

        // Phase 1: Auth selection
        let auth_methods = if self.username.is_some() {
            vec![Socks5AuthMethod::UserPass]
        } else {
            vec![Socks5AuthMethod::NoAuth]
        };

        let auth_select = Socks5AuthSelect {
            version: SOCKS5_VERSION,
            methods: auth_methods,
        };

        stream.write_all(&auth_select.to_bytes()).await?;

        // Read auth selection response
        let mut auth_response = [0u8; 2];
        stream.read_exact(&mut auth_response).await?;

        if auth_response[0] != SOCKS5_VERSION {
            return Err(anyhow!("Invalid SOCKS5 version in response"));
        }

        let selected_method = Socks5AuthMethod::from_byte(auth_response[1])?;

        // Phase 2: Authenticate if required
        if selected_method == Socks5AuthMethod::UserPass {
            self.do_username_password_auth(&mut stream).await?;
        }

        // Phase 3: Send CONNECT request
        let address = Socks5Address::from_socket_addr(target_addr);
        let request = Socks5Request {
            version: SOCKS5_VERSION,
            command: Socks5Command::Connect,
            reserved: 0x00,
            address,
        };

        stream.write_all(&request.to_bytes()).await?;

        // Read response
        let mut response_buf = [0u8; 512];
        let n = stream.read(&mut response_buf).await?;

        if n < 4 {
            return Err(anyhow!("SOCKS5 response too short"));
        }

        if response_buf[0] != SOCKS5_VERSION {
            return Err(anyhow!("Invalid SOCKS5 version in response"));
        }

        if response_buf[1] != 0x00 {
            let error = Socks5Error::from_reply_byte(response_buf[1]);
            return Err(anyhow!("SOCKS5 connect failed: {:?}", error));
        }

        println!("[socks5] Connected through proxy to {}", target_addr);

        Ok(stream)
    }

    /// Perform username/password authentication
    async fn do_username_password_auth(&self, stream: &mut TcpStream) -> Result<()> {
        let username = self.username.as_ref().ok_or_else(|| anyhow!("No username set"))?;
        let password = self.password.as_ref().ok_or_else(|| anyhow!("No password set"))?;

        let username_bytes = username.as_bytes();
        let password_bytes = password.as_bytes();

        let mut auth_packet = vec![0x01];  // Version
        auth_packet.push(username_bytes.len() as u8);
        auth_packet.extend_from_slice(username_bytes);
        auth_packet.push(password_bytes.len() as u8);
        auth_packet.extend_from_slice(password_bytes);

        stream.write_all(&auth_packet).await?;

        // Read auth response
        let mut auth_response = [0u8; 2];
        stream.read_exact(&mut auth_response).await?;

        if auth_response[1] != 0x00 {
            return Err(anyhow!("SOCKS5 authentication failed"));
        }

        println!("[socks5] Authentication successful");
        Ok(())
    }

    /// Connect to domain through SOCKS5 proxy
    pub async fn connect_domain(&self, domain: String, port: u16) -> Result<TcpStream> {
        // Connect to proxy
        let mut stream = TcpStream::connect(self.proxy_addr).await?;

        // Phase 1: Auth selection
        let auth_methods = if self.username.is_some() {
            vec![Socks5AuthMethod::UserPass]
        } else {
            vec![Socks5AuthMethod::NoAuth]
        };

        let auth_select = Socks5AuthSelect {
            version: SOCKS5_VERSION,
            methods: auth_methods,
        };

        stream.write_all(&auth_select.to_bytes()).await?;

        // Read auth selection response
        let mut auth_response = [0u8; 2];
        stream.read_exact(&mut auth_response).await?;

        let selected_method = Socks5AuthMethod::from_byte(auth_response[1])?;

        // Phase 2: Authenticate if required
        if selected_method == Socks5AuthMethod::UserPass {
            self.do_username_password_auth(&mut stream).await?;
        }

        // Phase 3: Send CONNECT request with domain
        let address = Socks5Address::Domain(domain, port);
        let request = Socks5Request {
            version: SOCKS5_VERSION,
            command: Socks5Command::Connect,
            reserved: 0x00,
            address,
        };

        stream.write_all(&request.to_bytes()).await?;

        // Read response
        let mut response_buf = [0u8; 512];
        let n = stream.read(&mut response_buf).await?;

        if n < 4 {
            return Err(anyhow!("SOCKS5 response too short"));
        }

        if response_buf[1] != 0x00 {
            let error = Socks5Error::from_reply_byte(response_buf[1]);
            return Err(anyhow!("SOCKS5 connect failed: {:?}", error));
        }

        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_address_conversion() {
        let addr = SocketAddr::from(([192, 168, 1, 1], 8080));
        let socks5_addr = Socks5Address::from_socket_addr(addr);

        match socks5_addr {
            Socks5Address::Ipv4(ip, port) => {
                assert_eq!(ip, Ipv4Addr::new(192, 168, 1, 1));
                assert_eq!(port, 8080);
            }
            _ => panic!("Not IPv4"),
        }
    }

    #[test]
    fn test_request_serialization() {
        let addr = Socks5Address::Ipv4(Ipv4Addr::new(127, 0, 0, 1), 9000);
        let request = Socks5Request {
            version: SOCKS5_VERSION,
            command: Socks5Command::Connect,
            reserved: 0x00,
            address: addr,
        };

        let bytes = request.to_bytes();
        assert_eq!(bytes[0], SOCKS5_VERSION);
        assert_eq!(bytes[1], Socks5Command::Connect.to_byte());
    }
}
