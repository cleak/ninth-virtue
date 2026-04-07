//! CLI tool to test the DOS memory scanner against a live DOSBox process.

use std::ffi::c_void;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};

fn find_dosbox_pid() -> Option<(u32, String)> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
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
                    let _ = CloseHandle(snapshot);
                    return Some((entry.th32ProcessID, name));
                }
                entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    None
}

struct DirectMemory {
    handle: HANDLE,
}

impl u5_companion::memory::access::MemoryAccess for DirectMemory {
    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> anyhow::Result<()> {
        unsafe {
            ReadProcessMemory(
                self.handle,
                addr as *const c_void,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                None,
            )?;
        }
        Ok(())
    }
    fn write_bytes(&self, _addr: usize, _data: &[u8]) -> anyhow::Result<()> {
        anyhow::bail!("write not supported in CLI tool")
    }
}

fn main() {
    use u5_companion::game::character::read_party;
    use u5_companion::game::inventory::read_inventory;
    use u5_companion::memory::access::Win32ProcessMemory;
    use u5_companion::memory::scanner;

    println!("=== DOSBox Memory Scanner Test ===\n");

    let (pid, name) = match find_dosbox_pid() {
        Some(p) => p,
        None => {
            eprintln!("No DOSBox process found!");
            return;
        }
    };
    println!("Found: {name} (PID {pid})");

    let handle = unsafe {
        match OpenProcess(
            PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION | PROCESS_QUERY_INFORMATION,
            false,
            pid,
        ) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("OpenProcess failed: {e}");
                return;
            }
        }
    };

    let mem = Win32ProcessMemory::new(handle);

    println!("\nRunning scanner...");
    match scanner::find_dos_base(&mem) {
        Ok(result) => {
            println!("SUCCESS: dos_base = {:#x}", result.dos_base);
            println!("  game_confirmed = {}", result.game_confirmed);

            if result.game_confirmed {
                // Use a separate read-only handle for game data
                let direct = DirectMemory {
                    handle: mem.handle(),
                };

                println!("\n--- Party ---");
                match read_party(&direct, result.dos_base) {
                    Ok(party) => {
                        for ch in &party {
                            println!(
                                "  {} ({:?}) - {:?} - HP {}/{} STR {} DEX {} INT {} MP {} XP {} Lvl {}",
                                ch.name,
                                ch.class,
                                ch.status,
                                ch.hp,
                                ch.max_hp,
                                ch.str_,
                                ch.dex,
                                ch.int,
                                ch.mp,
                                ch.xp,
                                ch.level
                            );
                        }
                    }
                    Err(e) => eprintln!("  Read party failed: {e}"),
                }

                println!("\n--- Inventory ---");
                match read_inventory(&direct, result.dos_base) {
                    Ok(inv) => {
                        println!(
                            "  Food={}, Gold={}, Keys={}, Gems={}",
                            inv.food, inv.gold, inv.keys, inv.gems
                        );
                        println!(
                            "  Torches={}, Arrows={}, Karma={}",
                            inv.torches, inv.arrows, inv.karma
                        );
                        println!("  Reagents: {:?}", inv.reagents);
                    }
                    Err(e) => eprintln!("  Read inventory failed: {e}"),
                }
            }
        }
        Err(e) => {
            eprintln!("FAILED: {e}");
        }
    }

    // Don't let Win32ProcessMemory close the handle since we're using it in DirectMemory too
    // Actually, DirectMemory doesn't own it, so this is fine
    std::mem::forget(mem);
    unsafe {
        let _ = CloseHandle(handle);
    }
}
