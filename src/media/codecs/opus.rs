//! Opus audio codec integration
//! 
//! Provides encoding/decoding for real-time voice communication

use std::sync::{Arc, Mutex};
use opus::{Channels, Bitrate, Bandwidth, Application};

/// Opus encoder for voice transmission
pub struct OpusEncoder {
    encoder: Arc<Mutex<opus::Encoder>>,
    sample_rate: u32,
    channels: u16,
    bitrate: u32,
}

impl OpusEncoder {
    /// Create new Opus encoder
    pub fn new(sample_rate: u32, channels: u16, bitrate: u32) -> Result<Self, String> {
        // Convert u16 to Channels enum
        let channels_enum = match channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            _ => return Err(format!("Unsupported channel count: {}", channels)),
        };
        
        let mut encoder = opus::Encoder::new(sample_rate, channels_enum, Application::Voip)
            .map_err(|e| format!("Failed to create Opus encoder: {}", e))?;
        
        // Set bitrate - convert u32 to i32
        let bitrate_i32 = bitrate as i32;
        encoder.set_bitrate(Bitrate::Bits(bitrate_i32))
            .map_err(|e| format!("Failed to set bitrate: {}", e))?;
        
        // Set bandwidth
        encoder.set_bandwidth(Bandwidth::Fullband)
            .map_err(|e| format!("Failed to set bandwidth: {}", e))?;
        
        Ok(Self {
            encoder: Arc::new(Mutex::new(encoder)),
            sample_rate,
            channels,
            bitrate,
        })
    }
    
    /// Encode PCM audio to Opus packet
    pub fn encode(&self, pcm: &[i16]) -> Result<Vec<u8>, String> {
        let mut encoder = self.encoder.lock().unwrap();
        
        // Calculate max output size (4 bytes per sample max)
        let max_size = pcm.len() * 4;
        let mut output = vec![0u8; max_size];
        
        let len = encoder.encode(pcm, &mut output)
            .map_err(|e| format!("Opus encode error: {}", e))?;
        
        output.truncate(len);
        Ok(output)
    }
    
    /// Get frame size for given duration (in samples)
    pub fn frame_size_samples(&self, ms: u32) -> usize {
        (self.sample_rate * ms / 1000) as usize * self.channels as usize
    }
}

/// Opus decoder for voice playback
pub struct OpusDecoder {
    decoder: Arc<Mutex<opus::Decoder>>,
    sample_rate: u32,
    channels: u16,
}

impl OpusDecoder {
    /// Create new Opus decoder
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, String> {
        // Convert u16 to Channels enum
        let channels_enum = match channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            _ => return Err(format!("Unsupported channel count: {}", channels)),
        };
        
        let decoder = opus::Decoder::new(sample_rate, channels_enum)
            .map_err(|e| format!("Failed to create Opus decoder: {}", e))?;
        
        Ok(Self {
            decoder: Arc::new(Mutex::new(decoder)),
            sample_rate,
            channels,
        })
    }
    
    /// Decode Opus packet to PCM
    pub fn decode(&self, packet: &[u8]) -> Result<Vec<i16>, String> {
        let mut decoder = self.decoder.lock().unwrap();
        let max_frame_size = (self.sample_rate * 120 / 1000) as usize * self.channels as usize;
        let mut output = vec![0i16; max_frame_size];
        
        let len = decoder.decode(packet, &mut output, false)
            .map_err(|e| format!("Opus decode error: {}", e))?;
        
        output.truncate(len);
        Ok(output)
    }
    
    /// Get PCM frame size for given packet (estimate)
    pub fn estimate_frame_size(&self, _packet: &[u8]) -> usize {
        (self.sample_rate * 20 / 1000) as usize * self.channels as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_opus_creation() {
        let encoder = OpusEncoder::new(48000, 1, 64000);
        assert!(encoder.is_ok());
        
        let decoder = OpusDecoder::new(48000, 1);
        assert!(decoder.is_ok());
    }
    
    #[test]
    fn test_encode_decode() {
        let encoder = OpusEncoder::new(48000, 1, 64000).unwrap();
        let decoder = OpusDecoder::new(48000, 1).unwrap();
        
        // Create silence frame (20ms at 48kHz mono = 960 samples)
        let pcm = vec![0i16; 960];
        
        let encoded = encoder.encode(&pcm).unwrap();
        assert!(!encoded.is_empty());
        
        let decoded = decoder.decode(&encoded).unwrap();
        assert_eq!(decoded.len(), pcm.len());
    }
}
