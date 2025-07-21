//! Key derivation and management for QUIC.
//!
//! This module provides key derivation and management for QUIC connections,
//! including initial keys, handshake keys, and application keys.

use crate::error::{H3Result, TlsError};
use crate::transport::ConnectionId;
use ring::hkdf;
use ring::hmac;
use ring::aead::{self, BoundKey, UnboundKey, Aad, Nonce, SealingKey, OpeningKey};

/// QUIC key phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    /// Initial keys
    Initial,
    /// Handshake keys
    Handshake,
    /// 0-RTT keys
    ZeroRTT,
    /// 1-RTT keys (phase 0)
    OneRTTPhase0,
    /// 1-RTT keys (phase 1)
    OneRTTPhase1,
}

/// QUIC encryption level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionLevel {
    /// Initial encryption
    Initial,
    /// 0-RTT encryption
    ZeroRTT,
    /// Handshake encryption
    Handshake,
    /// 1-RTT encryption
    OneRTT,
}

/// QUIC key set.
#[derive(Debug)]
pub struct KeySet {
    /// Encryption level
    pub level: EncryptionLevel,
    /// Key phase
    pub phase: KeyPhase,
    /// Client write key
    pub client_key: Vec<u8>,
    /// Server write key
    pub server_key: Vec<u8>,
    /// Client write IV
    pub client_iv: Vec<u8>,
    /// Server write IV
    pub server_iv: Vec<u8>,
    /// Header protection key
    pub hp_key: Vec<u8>,
}

impl KeySet {
    /// Create a new key set.
    pub fn new(level: EncryptionLevel, phase: KeyPhase) -> Self {
        Self {
            level,
            phase,
            client_key: Vec::new(),
            server_key: Vec::new(),
            client_iv: Vec::new(),
            server_iv: Vec::new(),
            hp_key: Vec::new(),
        }
    }
    
    /// Get the write key for the specified role.
    pub fn write_key(&self, is_client: bool) -> &[u8] {
        if is_client {
            &self.client_key
        } else {
            &self.server_key
        }
    }
    
    /// Get the read key for the specified role.
    pub fn read_key(&self, is_client: bool) -> &[u8] {
        if is_client {
            &self.server_key
        } else {
            &self.client_key
        }
    }
    
    /// Get the write IV for the specified role.
    pub fn write_iv(&self, is_client: bool) -> &[u8] {
        if is_client {
            &self.client_iv
        } else {
            &self.server_iv
        }
    }
    
    /// Get the read IV for the specified role.
    pub fn read_iv(&self, is_client: bool) -> &[u8] {
        if is_client {
            &self.server_iv
        } else {
            &self.client_iv
        }
    }
}

/// QUIC key derivation.
#[derive(Debug)]
pub struct KeyDerivation;

impl KeyDerivation {
    /// Derive initial keys.
    pub fn derive_initial_keys(connection_id: &ConnectionId) -> H3Result<KeySet> {
        // Initial salt (from QUIC v1)
        let initial_salt = &[
            0x38, 0x76, 0x2c, 0xf7, 0xf5, 0x59, 0x34, 0xb3,
            0x4d, 0x17, 0x9a, 0xe6, 0xa4, 0xc8, 0x0c, 0xad,
            0xcc, 0xbb, 0x7f, 0x0a,
        ];
        
        // Extract initial secret
        let initial_secret = Self::extract(initial_salt, connection_id.as_bytes())?;
        
        // Derive client and server secrets
        let client_secret = Self::expand_label(&initial_secret, b"client in", &[])?;
        let server_secret = Self::expand_label(&initial_secret, b"server in", &[])?;
        
        // Create key set
        let mut key_set = KeySet::new(EncryptionLevel::Initial, KeyPhase::Initial);
        
        // Derive client keys
        key_set.client_key = Self::expand_label(&client_secret, b"quic key", &[])?;
        key_set.client_iv = Self::expand_label(&client_secret, b"quic iv", &[])?;
        
        // Derive server keys
        key_set.server_key = Self::expand_label(&server_secret, b"quic key", &[])?;
        key_set.server_iv = Self::expand_label(&server_secret, b"quic iv", &[])?;
        
        // Derive header protection key
        key_set.hp_key = Self::expand_label(&initial_secret, b"quic hp", &[])?;
        
        Ok(key_set)
    }
    
    /// Extract a pseudorandom key.
    fn extract(salt: &[u8], ikm: &[u8]) -> H3Result<hmac::Key> {
        let salt = hmac::Key::new(hmac::HMAC_SHA256, salt);
        Ok(hkdf::extract(&salt, ikm))
    }
    
    /// Expand a label.
    fn expand_label(prk: &hmac::Key, label: &[u8], context: &[u8]) -> H3Result<Vec<u8>> {
        let mut info = Vec::with_capacity(2 + 1 + label.len() + 1 + context.len());
        
        // Length (u16, big-endian)
        info.push(0);
        info.push(32); // 32 bytes output
        
        // Label length and label
        info.push(label.len() as u8);
        info.extend_from_slice(label);
        
        // Context length and context
        info.push(context.len() as u8);
        info.extend_from_slice(context);
        
        let mut okm = vec![0u8; 32];
        hkdf::expand(prk, &info, &mut okm)
            .map_err(|_| TlsError::KeyDerivationFailed)?;
        
        Ok(okm)
    }
}

/// AEAD encryption/decryption.
#[derive(Debug)]
pub struct Aead {
    /// Encryption algorithm
    algorithm: &'static aead::Algorithm,
    /// Sealing key
    sealing_key: Option<SealingKey<AeadNonce>>,
    /// Opening key
    opening_key: Option<OpeningKey<AeadNonce>>,
}

impl Aead {
    /// Create a new AEAD instance.
    pub fn new() -> Self {
        Self {
            algorithm: &aead::AES_128_GCM,
            sealing_key: None,
            opening_key: None,
        }
    }
    
    /// Initialize with keys.
    pub fn init(&mut self, write_key: &[u8], read_key: &[u8]) -> H3Result<()> {
        // Create sealing key
        let sealing_key = UnboundKey::new(self.algorithm, write_key)
            .map_err(|_| TlsError::KeyDerivationFailed)?;
        self.sealing_key = Some(SealingKey::new(sealing_key, AeadNonce::default()));
        
        // Create opening key
        let opening_key = UnboundKey::new(self.algorithm, read_key)
            .map_err(|_| TlsError::KeyDerivationFailed)?;
        self.opening_key = Some(OpeningKey::new(opening_key, AeadNonce::default()));
        
        Ok(())
    }
    
    /// Seal (encrypt) data.
    pub fn seal(&mut self, packet_number: u64, data: &[u8], aad_data: &[u8]) -> H3Result<Vec<u8>> {
        let sealing_key = self.sealing_key.as_mut()
            .ok_or_else(|| TlsError::EncryptionFailed)?;
        
        // Update nonce with packet number
        sealing_key.nonce_mut().update(packet_number);
        
        // Create AAD
        let aad = Aad::from(aad_data);
        
        // Encrypt
        let mut in_out = data.to_vec();
        sealing_key.seal_in_place_append_tag(aad, &mut in_out)
            .map_err(|_| TlsError::EncryptionFailed)?;
        
        Ok(in_out)
    }
    
    /// Open (decrypt) data.
    pub fn open(&mut self, packet_number: u64, data: &[u8], aad_data: &[u8]) -> H3Result<Vec<u8>> {
        let opening_key = self.opening_key.as_mut()
            .ok_or_else(|| TlsError::DecryptionFailed)?;
        
        // Update nonce with packet number
        opening_key.nonce_mut().update(packet_number);
        
        // Create AAD
        let aad = Aad::from(aad_data);
        
        // Decrypt
        let mut in_out = data.to_vec();
        let result = opening_key.open_in_place(aad, &mut in_out)
            .map_err(|_| TlsError::DecryptionFailed)?;
        
        Ok(result.to_vec())
    }
}

impl Default for Aead {
    fn default() -> Self {
        Self::new()
    }
}

/// AEAD nonce.
#[derive(Debug)]
struct AeadNonce {
    /// Nonce bytes
    bytes: [u8; 12],
}

impl AeadNonce {
    /// Update the nonce with a packet number.
    fn update(&mut self, packet_number: u64) {
        let pn_bytes = packet_number.to_be_bytes();
        
        // XOR the last 8 bytes of the nonce with the packet number
        for i in 0..8 {
            self.bytes[4 + i] ^= pn_bytes[i];
        }
    }
}

impl Default for AeadNonce {
    fn default() -> Self {
        Self { bytes: [0; 12] }
    }
}

impl ring::aead::NonceSequence for AeadNonce {
    fn advance(&mut self) -> Result<Nonce, ring::error::Unspecified> {
        Ok(Nonce::assume_unique_for_key(self.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_key_derivation() {
        let connection_id = ConnectionId([1; 16]);
        let key_set = KeyDerivation::derive_initial_keys(&connection_id).unwrap();
        
        // Check that keys were derived
        assert!(!key_set.client_key.is_empty());
        assert!(!key_set.server_key.is_empty());
        assert!(!key_set.client_iv.is_empty());
        assert!(!key_set.server_iv.is_empty());
        assert!(!key_set.hp_key.is_empty());
    }
    
    #[test]
    fn test_aead() {
        let mut aead = Aead::new();
        
        // Initialize with test keys
        let write_key = vec![1; 16];
        let read_key = vec![2; 16];
        aead.init(&write_key, &read_key).unwrap();
        
        // Test data
        let data = b"Hello, QUIC!";
        let aad_data = b"AAD";
        let packet_number = 42;
        
        // Encrypt
        let encrypted = aead.seal(packet_number, data, aad_data).unwrap();
        
        // Decrypt (using a new AEAD instance to simulate the receiver)
        let mut aead2 = Aead::new();
        aead2.init(&read_key, &write_key).unwrap(); // Note: keys are swapped
        let decrypted = aead2.open(packet_number, &encrypted, aad_data).unwrap();
        
        // Check that decryption worked
        assert_eq!(decrypted, data);
    }
}