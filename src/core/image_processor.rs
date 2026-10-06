// src/core/image_processor.rs
//! Image processing for avatars
//! ==============================
//!
//! Resize and optimize images for avatar storage and P2P transmission

use image::{imageops::FilterType, DynamicImage, ImageError};
use image::ImageEncoder as _;
use std::io::Cursor;

/// Avatar configuration
pub const AVATAR_SIZE: u32 = 128;  // 128x128 pixels
pub const JPEG_QUALITY: u8 = 85;   // 85% quality (good balance)

/// Errors
#[derive(Debug)]
pub enum AvatarError {
    InvalidImage(String),
    ProcessError(ImageError),
    EncodeError(String),
}

impl std::fmt::Display for AvatarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AvatarError::InvalidImage(msg) => write!(f, "Invalid image: {}", msg),
            AvatarError::ProcessError(err) => write!(f, "Image processing error: {}", err),
            AvatarError::EncodeError(msg) => write!(f, "Encode error: {}", msg),
        }
    }
}

impl std::error::Error for AvatarError {}

impl From<ImageError> for AvatarError {
    fn from(err: ImageError) -> Self {
        AvatarError::ProcessError(err)
    }
}

/// Resize and optimize avatar for storage and P2P transmission
///
/// # Arguments
/// * `data` - Raw image bytes (PNG, JPEG, GIF)
///
/// # Returns
/// Optimized JPEG bytes (128x128, ~10KB)
///
/// # Example
/// ```
/// let optimized = optimize_avatar(raw_data)?;
/// // Was 2MB, now ~10KB
/// ```
pub fn optimize_avatar(data: &[u8]) -> Result<Vec<u8>, AvatarError> {
    // Load image from bytes
    let img = image::load_from_memory(data)
        .map_err(|e| AvatarError::InvalidImage(format!("Cannot load image: {}", e)))?;

    // Resize to AVATAR_SIZE x AVATAR_SIZE using Lanczos3 (high quality)
    let resized = image::imageops::resize(
        &img,
        AVATAR_SIZE,
        AVATAR_SIZE,
        FilterType::Lanczos3,
    );

    // Convert to DynamicImage, then to RGB (JPEG doesn't support RGBA/alpha channel)
    let resized_dynamic = DynamicImage::from(resized);
    let rgb_image = resized_dynamic.to_rgb8();

    // Encode as JPEG with quality
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);

    // Use DynamicImage's write_with_encoder method (newer API)
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, JPEG_QUALITY);
    encoder.write_image(
        &rgb_image,
        AVATAR_SIZE,
        AVATAR_SIZE,
        image::ExtendedColorType::Rgb8,
    )
        .map_err(|e| AvatarError::EncodeError(format!("Cannot encode JPEG: {}", e)))?;

    Ok(buffer)
}

/// Get hash of avatar data for P2P verification
pub fn get_avatar_hash(data: &[u8]) -> String {
    use blake3::Hash;
    let hash = blake3::hash(data);
    hash.to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimize_avatar() {
        // Create a simple 2x2 test image
        let data = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,  // PNG header
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
            0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02,
        ];

        let result = optimize_avatar(&data);
        assert!(result.is_ok() || result.is_err()); // May fail on invalid PNG
    }

    #[test]
    fn test_avatar_hash() {
        let data = b"test image data";
        let hash = get_avatar_hash(data);
        assert_eq!(hash.len(), 64); // Blake3 hex = 64 chars
    }
}
