//! Reads a `.safetensors` file: a little-endian u64 header length, a JSON
//! header with dtype, shape, and byte range per tensor, then the raw data.

use std::collections::HashMap;
use std::path::Path;

pub struct Tensors {
    bytes: Vec<u8>,
    data_start: usize,
    entries: HashMap<String, Entry>,
}

struct Entry {
    dtype: String,
    start: usize,
    end: usize,
}

impl Tensors {
    pub fn open(path: &Path) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        Self::parse(bytes)
    }

    pub fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        let header_len = bytes
            .get(..8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()) as usize)
            .ok_or("safetensors file is shorter than its header length")?;
        let data_start = 8usize
            .checked_add(header_len)
            .filter(|end| *end <= bytes.len())
            .ok_or("safetensors header runs past the end of the file")?;
        let header: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&bytes[8..data_start])
                .map_err(|e| format!("safetensors header is not JSON: {e}"))?;
        let mut entries = HashMap::new();
        for (name, value) in header {
            if name == "__metadata__" {
                continue;
            }
            let dtype = value["dtype"].as_str().unwrap_or_default().to_string();
            let offsets: Vec<usize> = value["data_offsets"]
                .as_array()
                .map(|o| {
                    o.iter()
                        .filter_map(|d| d.as_u64())
                        .map(|d| d as usize)
                        .collect()
                })
                .unwrap_or_default();
            let [start, end] = offsets[..] else {
                return Err(format!("tensor {name} has no data_offsets"));
            };
            if start > end || data_start + end > bytes.len() {
                return Err(format!("tensor {name} points outside the file"));
            }
            entries.insert(name, Entry { dtype, start, end });
        }
        Ok(Self {
            bytes,
            data_start,
            entries,
        })
    }

    fn entry(&self, name: &str) -> Result<&Entry, String> {
        self.entries
            .get(name)
            .ok_or_else(|| format!("model file has no tensor {name}"))
    }

    fn raw(&self, entry: &Entry) -> &[u8] {
        &self.bytes[self.data_start + entry.start..self.data_start + entry.end]
    }

    /// The tensor as f32 values, from F32, F16, or BF16 storage.
    pub fn f32(&self, name: &str) -> Result<Vec<f32>, String> {
        let entry = self.entry(name)?;
        let raw = self.raw(entry);
        let values = match entry.dtype.as_str() {
            "F32" => raw
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
            "F16" => raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| f16_to_f32(u16::from_le_bytes(*b)))
                .collect(),
            "BF16" => raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| f32::from_bits(u32::from(u16::from_le_bytes(*b)) << 16))
                .collect(),
            other => return Err(format!("tensor {name} has dtype {other}, not a float")),
        };
        Ok(values)
    }

    /// Checks the element count, so a wrong config cannot index past a tensor.
    pub fn f32_len(&self, name: &str, len: usize) -> Result<Vec<f32>, String> {
        let values = self.f32(name)?;
        if values.len() != len {
            return Err(format!(
                "tensor {name} has {} values, expected {len}",
                values.len()
            ));
        }
        Ok(values)
    }

    /// The raw bits of an F16 tensor with `len` values.
    pub fn f16_bits(&self, name: &str, len: usize) -> Result<Vec<u16>, String> {
        let entry = self.entry(name)?;
        if entry.dtype != "F16" {
            return Err(format!("tensor {name} has dtype {}, not F16", entry.dtype));
        }
        let bits: Vec<u16> = self
            .raw(entry)
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        if bits.len() != len {
            return Err(format!(
                "tensor {name} has {} values, expected {len}",
                bits.len()
            ));
        }
        Ok(bits)
    }

    pub fn u8(&self, name: &str) -> Result<&[u8], String> {
        let entry = self.entry(name)?;
        if entry.dtype != "U8" {
            return Err(format!("tensor {name} has dtype {}, not U8", entry.dtype));
        }
        Ok(self.raw(entry))
    }
}

/// IEEE 754 half precision to single precision, including subnormals.
pub fn f16_to_f32(half: u16) -> f32 {
    let sign = u32::from(half >> 15) << 31;
    let exponent = u32::from((half >> 10) & 0x1f);
    let mantissa = u32::from(half & 0x3ff);
    let bits = match (exponent, mantissa) {
        (0, 0) => sign,
        (0, _) => {
            // Subnormal: value is mantissa * 2^-24. Normalize it.
            let mut exp = 127 - 15 + 1;
            let mut m = mantissa;
            while m & 0x400 == 0 {
                m <<= 1;
                exp -= 1;
            }
            sign | (exp << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, _) => sign | 0x7f80_0000 | (mantissa << 13),
        _ => sign | ((exponent + 127 - 15) << 23) | (mantissa << 13),
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::{f16_to_f32, Tensors};

    #[test]
    fn half_precision_values_convert_exactly() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(f16_to_f32(0x7bff), 65504.0);
        assert!(f16_to_f32(0x7c00).is_infinite());
        assert_eq!(f16_to_f32(0x8000).to_bits(), (-0.0f32).to_bits());
    }

    #[test]
    fn a_small_file_parses_and_reads_typed_values() {
        let header = br#"{"a":{"dtype":"F16","shape":[2],"data_offsets":[0,4]},"b":{"dtype":"U8","shape":[3],"data_offsets":[4,7]},"__metadata__":{}}"#;
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header);
        bytes.extend_from_slice(&[0x00, 0x3c, 0x00, 0xc0, 7, 8, 9]);
        let tensors = Tensors::parse(bytes).expect("parse");
        assert_eq!(tensors.f32("a").unwrap(), vec![1.0, -2.0]);
        assert_eq!(tensors.u8("b").unwrap(), &[7, 8, 9]);
        assert!(tensors.f32("missing").is_err());
        assert!(tensors.f32_len("a", 3).is_err());
    }
}
