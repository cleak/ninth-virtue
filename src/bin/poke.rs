//! CLI tool to read/write arbitrary memory in a live DOSBox process.
//!
//! ```text
//! cargo run --bin poke -- read 0x2D5
//! cargo run --bin poke -- write 0x2D5 0x01
//! cargo run --bin poke -- dump 0x2D0 32
//! cargo run --bin poke -- read --abs 0x24826
//! ```

use ninth_virtue::game::offsets::{self, SAVE_BASE};
use ninth_virtue::memory::access::MemoryAccess;
use ninth_virtue::memory::{process, scanner};

fn parse_hex_or_dec(s: &str) -> Result<usize, String> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        usize::from_str_radix(hex, 16).map_err(|e| format!("bad hex {s}: {e}"))
    } else {
        s.parse::<usize>()
            .map_err(|e| format!("bad number {s}: {e}"))
    }
}

fn attach() -> (impl MemoryAccess, usize) {
    let procs = process::list_dosbox_processes().expect("process scan failed");
    assert!(!procs.is_empty(), "no DOSBox process found");
    let (pid, name) = &procs[0];
    eprintln!("Attaching to {name} (PID {pid})...");
    let proc = process::attach(*pid).expect("attach failed");
    let result = scanner::find_dos_base(&proc.memory).expect("scanner failed");
    assert!(result.game_confirmed, "game not loaded — load a save first");
    eprintln!("dos_base = {:#x}", result.dos_base);
    (proc.memory, result.dos_base)
}

fn resolve_addr(dos_base: usize, offset_str: &str, absolute: bool) -> usize {
    let offset = parse_hex_or_dec(offset_str).expect("invalid offset");
    if absolute {
        dos_base + offset
    } else {
        offsets::inv_addr(dos_base, offset)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: poke <read|write|dump> [--abs] <offset> [value|len]");
        eprintln!();
        eprintln!("  read  <offset>        Read one byte (save-relative)");
        eprintln!("  write <offset> <val>  Write one byte");
        eprintln!("  dump  <offset> <len>  Hex dump a range");
        eprintln!();
        eprintln!("  --abs  Treat offset as absolute DOS address (not save-relative)");
        eprintln!();
        eprintln!("Offsets accept hex (0x1FF) or decimal (511).");
        std::process::exit(1);
    }

    let cmd = args[1].as_str();

    // Parse --abs flag
    let abs_flag = args.iter().any(|a| a == "--abs");
    let positional: Vec<&str> = args[2..]
        .iter()
        .filter(|a| *a != "--abs")
        .map(|s| s.as_str())
        .collect();

    let (mem, dos_base) = attach();

    match cmd {
        "read" => {
            assert!(!positional.is_empty(), "read requires <offset>");
            let addr = resolve_addr(dos_base, positional[0], abs_flag);
            let val = mem.read_u8(addr).expect("read failed");
            let save_off = addr.wrapping_sub(dos_base + SAVE_BASE);
            let label = offsets::label_for_save_offset(save_off).unwrap_or("???");
            println!(
                "[{:#07X}] save+{:#05X} ({}) = {:#04X} ({})",
                addr, save_off, label, val, val
            );
        }
        "write" => {
            assert!(positional.len() >= 2, "write requires <offset> <value>");
            let addr = resolve_addr(dos_base, positional[0], abs_flag);
            let val = parse_hex_or_dec(positional[1]).expect("invalid value") as u8;
            let old = mem.read_u8(addr).expect("read failed");
            mem.write_u8(addr, val).expect("write failed");
            let save_off = addr.wrapping_sub(dos_base + SAVE_BASE);
            let label = offsets::label_for_save_offset(save_off).unwrap_or("???");
            println!(
                "[{:#07X}] save+{:#05X} ({}) {:#04X} -> {:#04X}",
                addr, save_off, label, old, val
            );
        }
        "dump" => {
            assert!(positional.len() >= 2, "dump requires <offset> <len>");
            let addr = resolve_addr(dos_base, positional[0], abs_flag);
            let len = parse_hex_or_dec(positional[1]).expect("invalid length");
            let mut buf = vec![0u8; len];
            mem.read_bytes(addr, &mut buf).expect("read failed");

            for (i, chunk) in buf.chunks(16).enumerate() {
                let row_addr = addr + i * 16;
                let save_off = row_addr.wrapping_sub(dos_base + SAVE_BASE);
                print!("{:#07X} save+{:#05X} | ", row_addr, save_off);
                for (j, byte) in chunk.iter().enumerate() {
                    if j == 8 {
                        print!(" ");
                    }
                    print!("{:02X} ", byte);
                }
                // Pad short last row
                for _ in chunk.len()..16 {
                    print!("   ");
                }
                print!("| ");
                for byte in chunk {
                    let c = if byte.is_ascii_graphic() || *byte == b' ' {
                        *byte as char
                    } else {
                        '.'
                    };
                    print!("{c}");
                }
                println!();
            }
        }
        _ => {
            eprintln!("Unknown command: {cmd}");
            eprintln!("Use: read, write, dump");
            std::process::exit(1);
        }
    }
}
