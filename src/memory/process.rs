use anyhow::{Context, Result};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION,
    PROCESS_VM_READ, PROCESS_VM_WRITE,
};

use super::access::Win32ProcessMemory;

const STILL_ACTIVE: u32 = 259;

#[allow(dead_code)]
pub struct DosBoxProcess {
    pub pid: u32,
    pub name: String,
    pub memory: Win32ProcessMemory,
}

impl DosBoxProcess {
    pub fn is_alive(&self) -> bool {
        let mut exit_code: u32 = 0;
        unsafe {
            GetExitCodeProcess(self.memory.handle(), &mut exit_code).is_ok()
                && exit_code == STILL_ACTIVE
        }
    }
}

pub fn list_dosbox_processes() -> Result<Vec<(u32, String)>> {
    let mut results = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .context("CreateToolhelp32Snapshot failed")?;

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name_len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);

                if name.to_lowercase().contains("dosbox") {
                    results.push((entry.th32ProcessID, name));
                }

                entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
    }
    Ok(results)
}

pub fn attach(pid: u32) -> Result<DosBoxProcess> {
    let processes = list_dosbox_processes()?;
    let (_, name) = processes
        .into_iter()
        .find(|(p, _)| *p == pid)
        .with_context(|| format!("process {pid} not found"))?;

    let handle: HANDLE = unsafe {
        OpenProcess(
            PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION | PROCESS_QUERY_INFORMATION,
            false,
            pid,
        )
        .context("OpenProcess failed")?
    };

    Ok(DosBoxProcess {
        pid,
        name,
        memory: Win32ProcessMemory::new(handle),
    })
}
