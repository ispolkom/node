//! Audio capture and playback using cpal

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat, StreamConfig,
};
use std::sync::mpsc;
use std::thread;

/// Audio capture configuration
#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_size: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 1,
            buffer_size: 960, // 20ms at 48kHz
        }
    }
}

/// Audio capture device
pub struct AudioCapture {
    _stream: cpal::Stream,
    rx: mpsc::Receiver<Vec<i16>>,
}

impl AudioCapture {
    pub fn new(config: AudioConfig) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host.default_input_device()
            .ok_or("No input device available")?;
        
        let supported_config = device.default_input_config()
            .map_err(|e| format!("Failed to get default config: {}", e))?;
        
        let (tx, rx) = mpsc::channel();
        
        let stream = device.build_input_stream(
            &supported_config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Convert f32 to i16 and send
                let samples: Vec<i16> = data.iter()
                    .map(|&s| (s * i16::MAX as f32) as i16)
                    .collect();
                let _ = tx.send(samples);
            },
            move |err| {
                eprintln!("Audio capture error: {}", err);
            },
            None,
        ).map_err(|e| format!("Failed to build stream: {}", e))?;
        
        stream.play().map_err(|e| format!("Failed to start stream: {}", e))?;
        
        Ok(Self {
            _stream: stream,
            rx,
        })
    }
    
    pub fn read_samples(&self) -> Option<Vec<i16>> {
        self.rx.try_recv().ok()
    }
}

/// Audio playback device
pub struct AudioPlayback {
    _stream: cpal::Stream,
    tx: mpsc::SyncSender<Vec<i16>>,
}

impl AudioPlayback {
    pub fn new(config: AudioConfig) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or("No output device available")?;
        
        let supported_config = device.default_output_config()
            .map_err(|e| format!("Failed to get default config: {}", e))?;
        
        let (tx, rx): (mpsc::SyncSender<Vec<i16>>, mpsc::Receiver<Vec<i16>>) = mpsc::sync_channel(32);
        
        let stream = device.build_output_stream(
            &supported_config.into(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                if let Ok(samples) = rx.try_recv() {
                    for (i, sample) in samples.iter().enumerate() {
                        if i < data.len() {
                            data[i] = *sample as f32 / i16::MAX as f32;
                        }
                    }
                } else {
                    // Silence
                    for sample in data.iter_mut() {
                        *sample = 0.0;
                    }
                }
            },
            move |err| {
                eprintln!("Audio playback error: {}", err);
            },
            None,
        ).map_err(|e| format!("Failed to build stream: {}", e))?;
        
        stream.play().map_err(|e| format!("Failed to start stream: {}", e))?;
        
        Ok(Self {
            _stream: stream,
            tx,
        })
    }
    
    pub fn play_samples(&self, samples: Vec<i16>) -> Result<(), String> {
        self.tx.send(samples).map_err(|e| format!("Failed to queue samples: {}", e))?;
        Ok(())
    }
}
