use anyhow::{Result, bail};

use crate::memory::access::MemoryAccess;

#[derive(Debug, Clone, Default)]
pub struct BufferedMemory {
    ranges: Vec<BufferedRange>,
}

#[derive(Debug, Clone)]
struct BufferedRange {
    start: usize,
    bytes: Vec<u8>,
}

impl BufferedMemory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_range(&mut self, start: usize, bytes: Vec<u8>) {
        self.ranges.push(BufferedRange { start, bytes });
    }

    pub fn capture_range(
        &mut self,
        mem: &dyn MemoryAccess,
        start: usize,
        len: usize,
    ) -> Result<()> {
        let mut bytes = vec![0u8; len];
        mem.read_bytes(start, &mut bytes)?;
        self.push_range(start, bytes);
        Ok(())
    }

    pub fn capture_optional_range(&mut self, mem: &dyn MemoryAccess, start: usize, len: usize) {
        let _ = self.capture_range(mem, start, len);
    }
}

impl MemoryAccess for BufferedMemory {
    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> Result<()> {
        let Some(end) = addr.checked_add(buf.len()) else {
            bail!("buffered read overflow at {addr:#x}+{}", buf.len());
        };

        for range in &self.ranges {
            let Some(range_end) = range.start.checked_add(range.bytes.len()) else {
                continue;
            };
            if addr >= range.start && end <= range_end {
                let offset = addr - range.start;
                buf.copy_from_slice(&range.bytes[offset..offset + buf.len()]);
                return Ok(());
            }
        }

        bail!("buffered range missing for read at {addr:#x}+{}", buf.len());
    }

    fn write_bytes(&self, addr: usize, data: &[u8]) -> Result<()> {
        bail!(
            "buffered snapshot is read-only (attempted write at {addr:#x}+{})",
            data.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_from_any_contained_range() {
        let mut mem = BufferedMemory::new();
        mem.push_range(0x1000, vec![1, 2, 3, 4]);
        mem.push_range(0x2000, vec![5, 6, 7, 8]);

        let mut left = [0u8; 2];
        mem.read_bytes(0x1001, &mut left).unwrap();
        assert_eq!(left, [2, 3]);

        let mut right = [0u8; 3];
        mem.read_bytes(0x2000, &mut right).unwrap();
        assert_eq!(right, [5, 6, 7]);
    }

    #[test]
    fn rejects_reads_outside_loaded_ranges() {
        let mut mem = BufferedMemory::new();
        mem.push_range(0x1000, vec![1, 2, 3, 4]);

        let err = mem.read_bytes(0x1003, &mut [0u8; 2]).unwrap_err();
        assert!(
            err.to_string()
                .contains("buffered range missing for read at 0x1003+2")
        );
    }

    #[test]
    fn capture_range_reads_from_source_memory() {
        let source = crate::memory::access::MockMemory::new(0x3000);
        source.set_bytes(0x1200, &[9, 8, 7, 6]);

        let mut snapshot = BufferedMemory::new();
        snapshot.capture_range(&source, 0x1200, 4).unwrap();

        let mut buf = [0u8; 4];
        snapshot.read_bytes(0x1200, &mut buf).unwrap();
        assert_eq!(buf, [9, 8, 7, 6]);
    }
}
