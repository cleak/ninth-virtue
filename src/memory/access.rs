use anyhow::Result;

pub trait MemoryAccess {
    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> Result<()>;
    fn write_bytes(&self, addr: usize, data: &[u8]) -> Result<()>;

    fn read_u8(&self, addr: usize) -> Result<u8> {
        let mut buf = [0u8; 1];
        self.read_bytes(addr, &mut buf)?;
        Ok(buf[0])
    }

    fn read_u16_le(&self, addr: usize) -> Result<u16> {
        let mut buf = [0u8; 2];
        self.read_bytes(addr, &mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }

    fn write_u8(&self, addr: usize, val: u8) -> Result<()> {
        self.write_bytes(addr, &[val])
    }

    fn write_u16_le(&self, addr: usize, val: u16) -> Result<()> {
        self.write_bytes(addr, &val.to_le_bytes())
    }
}

// --- Win32 implementation ---

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};

pub struct Win32ProcessMemory {
    handle: HANDLE,
}

impl Win32ProcessMemory {
    pub fn new(handle: HANDLE) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> HANDLE {
        self.handle
    }
}

impl MemoryAccess for Win32ProcessMemory {
    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> Result<()> {
        unsafe {
            ReadProcessMemory(
                self.handle,
                addr as *const std::ffi::c_void,
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                buf.len(),
                None,
            )?;
        }
        Ok(())
    }

    fn write_bytes(&self, addr: usize, data: &[u8]) -> Result<()> {
        unsafe {
            WriteProcessMemory(
                self.handle,
                addr as *const std::ffi::c_void,
                data.as_ptr() as *const std::ffi::c_void,
                data.len(),
                None,
            )?;
        }
        Ok(())
    }
}

impl Drop for Win32ProcessMemory {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

// --- Mock implementation for tests ---

#[cfg(test)]
pub struct MockMemory {
    data: std::cell::RefCell<Vec<u8>>,
}

#[cfg(test)]
impl MockMemory {
    pub fn new(size: usize) -> Self {
        Self {
            data: std::cell::RefCell::new(vec![0; size]),
        }
    }

    pub fn set_bytes(&self, addr: usize, bytes: &[u8]) {
        let mut data = self.data.borrow_mut();
        data[addr..addr + bytes.len()].copy_from_slice(bytes);
    }
}

#[cfg(test)]
impl MemoryAccess for MockMemory {
    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> Result<()> {
        let data = self.data.borrow();
        let end = addr + buf.len();
        anyhow::ensure!(
            end <= data.len(),
            "read at {addr:#x}+{} out of bounds (size={})",
            buf.len(),
            data.len()
        );
        buf.copy_from_slice(&data[addr..end]);
        Ok(())
    }

    fn write_bytes(&self, addr: usize, data: &[u8]) -> Result<()> {
        let mut storage = self.data.borrow_mut();
        let end = addr + data.len();
        anyhow::ensure!(
            end <= storage.len(),
            "write at {addr:#x}+{} out of bounds (size={})",
            data.len(),
            storage.len()
        );
        storage[addr..end].copy_from_slice(data);
        Ok(())
    }
}
