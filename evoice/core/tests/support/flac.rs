#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]
//! A FLAC file of pure silence, built by hand: one constant subframe per 4096-sample block, so an hour of audio
//! is a few hundred kilobytes. The decompression bomb the services must refuse without decoding it whole.

const BLOCK: u32 = 4_096;

fn crc8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |mut crc, byte| {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 == 0 { crc << 1 } else { (crc << 1) ^ 0x07 };
        }
        crc
    })
}

fn crc16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0u16, |mut crc, byte| {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x8005
            };
        }
        crc
    })
}

/// FLAC's UTF-8-style coding of the frame number.
fn frame_number(number: u32) -> Vec<u8> {
    match number {
        0..=0x7F => vec![number as u8],
        0x80..=0x7FF => vec![0xC0 | (number >> 6) as u8, 0x80 | (number & 0x3F) as u8],
        0x800..=0xFFFF => vec![
            0xE0 | (number >> 12) as u8,
            0x80 | ((number >> 6) & 0x3F) as u8,
            0x80 | (number & 0x3F) as u8,
        ],
        _ => vec![
            0xF0 | (number >> 18) as u8,
            0x80 | ((number >> 12) & 0x3F) as u8,
            0x80 | ((number >> 6) & 0x3F) as u8,
            0x80 | (number & 0x3F) as u8,
        ],
    }
}

/// Mono 16-bit silence of `seconds` at `rate` Hz, rounded up to whole 4096-sample blocks.
pub fn silent_flac(seconds: u32, rate: u32) -> Vec<u8> {
    let blocks = (u64::from(seconds) * u64::from(rate)).div_ceil(u64::from(BLOCK)) as u32;
    let total = u64::from(blocks) * u64::from(BLOCK);
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0, 0, 34]);
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    let packed: u64 = (u64::from(rate) << 44) | (15u64 << 36) | total;
    out.extend_from_slice(&packed.to_be_bytes());
    out.extend_from_slice(&[0; 16]);
    for number in 0..blocks {
        let mut frame = vec![0xFF, 0xF8, 0xC0, 0x08];
        frame.extend(frame_number(number));
        frame.push(crc8(&frame));
        frame.extend_from_slice(&[0x00, 0x00, 0x00]);
        let crc = crc16(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        out.extend(frame);
    }
    out
}
