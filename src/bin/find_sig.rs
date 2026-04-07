//! Scan the full DOSBox memory region for the redraw_full_stats signature.

fn main() {
    use ninth_virtue::memory::access::MemoryAccess;
    use ninth_virtue::memory::{process, scanner};

    let procs = process::list_dosbox_processes().unwrap();
    assert!(!procs.is_empty(), "no DOSBox process");
    let (pid, name) = &procs[0];
    eprintln!("Attaching to {name} (PID {pid})...");
    let proc = process::attach(*pid).unwrap();
    let result = scanner::find_dos_base(&proc.memory).unwrap();
    let dos_base = result.dos_base;
    eprintln!("dos_base = {dos_base:#x}");

    let sig: [u8; 7] = [0x55, 0x8B, 0xEC, 0x83, 0xEC, 0x02, 0x56];
    let loop_jmp: [u8; 3] = [0xE9, 0x41, 0xFF];

    // Scan in 1MB chunks, up to 32MB from dos_base
    let chunk_size = 0x10_0000usize; // 1MB
    let max_scan = 0x200_0000usize; // 32MB

    eprintln!("Scanning {max_scan:#x} bytes for signature {sig:02X?}...");

    let mut found = 0u32;
    for chunk_start in (0..max_scan).step_by(chunk_size) {
        let mut buf = vec![0u8; chunk_size];
        if proc
            .memory
            .read_bytes(dos_base + chunk_start, &mut buf)
            .is_err()
        {
            eprintln!("  Chunk at +{chunk_start:#x}: read failed, stopping");
            break;
        }

        for i in 0..buf.len().saturating_sub(sig.len()) {
            if buf[i..i + sig.len()] == sig {
                let dos_offset = chunk_start + i;
                // Check if this could be CS:0x2900 by validating CS:0x0174
                if dos_offset >= 0x2900 {
                    let candidate_cs = dos_offset - 0x2900;
                    let jmp_offset = candidate_cs + 0x0174;

                    // Read the JMP bytes
                    if jmp_offset + 3 <= chunk_start + buf.len() {
                        let local = jmp_offset - chunk_start;
                        let jmp_bytes = &buf[local..local + 3];
                        let jmp_match = jmp_bytes == loop_jmp;

                        if jmp_match {
                            println!(
                                "MATCH: dos+{dos_offset:#x} (CS base = dos+{candidate_cs:#x}) — \
                                 JMP at CS:0x0174 = {jmp_bytes:02X?} ✓"
                            );
                        } else {
                            println!(
                                "  sig at dos+{dos_offset:#x} (CS base = dos+{candidate_cs:#x}) — \
                                 JMP at CS:0x0174 = {jmp_bytes:02X?} ✗"
                            );
                        }
                    } else {
                        // JMP is in a different chunk, read it separately
                        let mut jmp_buf = [0u8; 3];
                        if proc
                            .memory
                            .read_bytes(dos_base + jmp_offset, &mut jmp_buf)
                            .is_ok()
                        {
                            let jmp_match = jmp_buf == loop_jmp;
                            if jmp_match {
                                println!(
                                    "MATCH: dos+{dos_offset:#x} (CS base = dos+{candidate_cs:#x}) — \
                                     JMP at CS:0x0174 = {jmp_buf:02X?} ✓"
                                );
                            }
                        }
                    }
                }
                found += 1;
            }
        }
    }

    eprintln!("Done. {found} signature occurrences found.");
}
