//! LZW decompressor for Origin Systems (Ultima V/VI) compressed files.
//!
//! Format: GIF-style variable-width LZW with LSB-first bit packing.
//! File layout: `u32le uncompressed_length | compressed_data[]`

use anyhow::{Result, ensure};

const CLEAR_CODE: u16 = 0x100;
const END_CODE: u16 = 0x101;
const FIRST_ENTRY: u16 = 0x102;

const INITIAL_CODE_SIZE: u32 = 9;
const MAX_CODE_SIZE: u32 = 12;
const MAX_DICT_SIZE: usize = 1 << MAX_CODE_SIZE; // 4096

/// Decompress an Origin Systems LZW-compressed file.
///
/// Input format: 4-byte LE uncompressed length, followed by compressed data.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() >= 4, "LZW data too short for header");

    let uncompressed_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    ensure!(
        uncompressed_len <= 16 * 1024 * 1024,
        "uncompressed length {uncompressed_len} too large"
    );

    let compressed = &data[4..];
    let mut output = Vec::with_capacity(uncompressed_len);

    // Dictionary: each entry is (prefix_code, append_byte).
    // For codes 0-255, the entry is the literal byte itself.
    // We store the full decoded string lazily via recursion through prefix chains.
    let mut dict_prefix = vec![0u16; MAX_DICT_SIZE];
    let mut dict_byte = vec![0u8; MAX_DICT_SIZE];
    let mut dict_len = vec![0u16; MAX_DICT_SIZE]; // length of decoded string

    let mut code_size = INITIAL_CODE_SIZE;
    let mut dict_size = 1u32 << code_size; // 512
    let mut next_free = FIRST_ENTRY;
    let mut bits_read = 0u32;
    let mut prev_code: Option<u16> = None;

    // Initialize literal entries
    for i in 0u16..256 {
        dict_byte[i as usize] = i as u8;
        dict_len[i as usize] = 1;
    }

    loop {
        // Read next code from the bitstream (LSB-first)
        let code = read_code(compressed, bits_read, code_size);
        bits_read += code_size;

        if code == END_CODE {
            break;
        }

        if code == CLEAR_CODE {
            code_size = INITIAL_CODE_SIZE;
            dict_size = 1 << code_size;
            next_free = FIRST_ENTRY;
            prev_code = None;
            continue;
        }

        // Determine the string to output
        let is_known = code < next_free;
        let decode_code = if is_known {
            code
        } else {
            // KwKwK case: code == next_free, decode prev_code and append its first byte
            prev_code.ok_or_else(|| {
                anyhow::anyhow!("invalid LZW stream: unknown code {code} with no previous code")
            })?
        };

        // Decode the string for decode_code by walking the prefix chain
        let string_len = dict_len[decode_code as usize] as usize;
        let out_start = output.len();

        // Reserve space and decode backwards
        output.resize(out_start + string_len, 0);
        let mut c = decode_code;
        for i in (0..string_len).rev() {
            output[out_start + i] = dict_byte[c as usize];
            c = dict_prefix[c as usize];
        }

        if !is_known {
            // KwKwK: append the first byte of the decoded string
            output.push(output[out_start]);
        }

        // Add new dictionary entry: prev_code + first byte of current output
        if let Some(prev) = prev_code
            && (next_free as usize) < MAX_DICT_SIZE
        {
            let first_byte = output[out_start];
            dict_prefix[next_free as usize] = prev;
            dict_byte[next_free as usize] = first_byte;
            dict_len[next_free as usize] = dict_len[prev as usize] + 1;
            next_free += 1;

            // Grow code size when dictionary reaches current capacity
            if next_free as u32 >= dict_size && code_size < MAX_CODE_SIZE {
                code_size += 1;
                dict_size = 1 << code_size;
            }
        }

        prev_code = Some(code);

        if output.len() >= uncompressed_len {
            break;
        }
    }

    output.truncate(uncompressed_len);
    Ok(output)
}

/// Read a variable-width code from the bitstream at the given bit offset.
/// LSB-first packing (like GIF).
fn read_code(data: &[u8], bits_read: u32, code_size: u32) -> u16 {
    let byte_offset = (bits_read / 8) as usize;
    let bit_offset = bits_read % 8;

    // Read up to 3 bytes to cover codes spanning byte boundaries
    let b0 = *data.get(byte_offset).unwrap_or(&0) as u32;
    let b1 = *data.get(byte_offset + 1).unwrap_or(&0) as u32;
    let b2 = if code_size + bit_offset > 16 {
        *data.get(byte_offset + 2).unwrap_or(&0) as u32
    } else {
        0
    };

    let combined = b0 | (b1 << 8) | (b2 << 16);
    let shifted = combined >> bit_offset;
    let mask = (1u32 << code_size) - 1;
    (shifted & mask) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_too_short() {
        assert!(decompress(&[0, 0, 0]).is_err());
    }

    #[test]
    fn decompress_trivial_end_code() {
        // Header: uncompressed length = 0, then END_CODE (0x101) as 9-bit LSB-first.
        // 0x101 in binary = 100000001. LSB-first into bytes:
        //   byte[0] = bits 0-7 = 0b00000001 = 0x01
        //   byte[1] = bit 8    = 0b00000001 = 0x01
        let data = vec![0, 0, 0, 0, 0x01, 0x01];
        let result = decompress(&data).unwrap();
        assert_eq!(result.len(), 0);
    }
}
