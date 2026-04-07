//! Interactive memory-diff tool for finding game-engine dirty flags.
//!
//! Scans full DOS conventional memory (640 KB) with three phases:
//!
//! 1. **Baseline** — poll while game is idle, mark all changing addresses
//!    as "noisy" so they're ignored in later phases.
//!
//! 2. **Monitor** — continuously poll while you perform an action in-game.
//!    Any non-noisy address that changes is recorded as a candidate.
//!
//! 3. **Confirm** — after the screen has updated, check which candidates
//!    returned to their original (pre-action) value.  Those are likely
//!    dirty flags (set to trigger redraw, then cleared by the engine).
//!
//! Repeat phases 2–3 to narrow candidates further.
//!
//! ```text
//! cargo run --release --bin memdiff
//! ```

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use ninth_virtue::game::offsets::{self, SAVE_BASE};
use ninth_virtue::memory::access::MemoryAccess;
use ninth_virtue::memory::{process, scanner};

/// Size of DOS conventional memory.
const DOS_MEM_SIZE: usize = 0xA0000; // 640 KB

/// Interval between baseline snapshots.
const BASELINE_INTERVAL: Duration = Duration::from_millis(100);

/// Interval between monitor-phase polls.
const MONITOR_INTERVAL: Duration = Duration::from_millis(5);

fn snapshot(mem: &dyn MemoryAccess, dos_base: usize) -> Vec<u8> {
    let mut buf = vec![0u8; DOS_MEM_SIZE];
    mem.read_bytes(dos_base, &mut buf)
        .expect("ReadProcessMemory failed on DOS memory");
    buf
}

fn wait_for_enter(prompt: &str) {
    eprint!("{prompt}");
    io::stderr().flush().unwrap();
    let mut line = String::new();
    io::stdin().read_line(&mut line).unwrap();
}

fn format_addr(i: usize, dos_base: usize) -> String {
    let abs = dos_base + i;
    if i >= SAVE_BASE {
        let save_off = i - SAVE_BASE;
        let label = offsets::label_for_save_offset(save_off).unwrap_or("???");
        format!("DOS:{abs:#07X}  save+{save_off:#05X}  {label:>16}")
    } else {
        format!("DOS:{abs:#07X}  {:>28}", "")
    }
}

fn main() {
    eprintln!("=== Memory Diff Tool ===");
    eprintln!();
    eprintln!("IMPORTANT: Downclock DOSBox first! (Ctrl+F11 repeatedly, or set cycles=50)");
    eprintln!("           This ensures transient flags are visible between polls.");
    eprintln!();

    // --- Attach ---
    let procs = process::list_dosbox_processes().expect("process scan failed");
    assert!(!procs.is_empty(), "no DOSBox process found");
    let (pid, name) = &procs[0];
    eprintln!("Attaching to {name} (PID {pid})...");
    let proc = process::attach(*pid).expect("attach failed");
    let result = scanner::find_dos_base(&proc.memory).expect("scanner failed");
    assert!(result.game_confirmed, "game not loaded — load a save first");
    let dos_base = result.dos_base;
    eprintln!("dos_base = {dos_base:#x}");
    eprintln!("Scanning {DOS_MEM_SIZE} bytes (640 KB) from dos_base");
    eprintln!();

    let mem: &dyn MemoryAccess = &proc.memory;

    // ================================================================
    // Phase 1: Baseline — identify noisy addresses
    // ================================================================
    wait_for_enter("Press ENTER to start baseline capture (keep the game IDLE)...");
    eprintln!("Capturing baseline for 5 seconds...");

    let mut noisy: HashSet<usize> = HashSet::new();
    let mut prev = snapshot(mem, dos_base);
    let baseline_start = Instant::now();
    let mut snap_count = 0u32;

    // Poll in a loop until stdin has a line ready.
    // We use a non-blocking approach: poll stdin in between snapshots.
    loop {
        thread::sleep(BASELINE_INTERVAL);
        let cur = snapshot(mem, dos_base);
        snap_count += 1;
        let mut new_noisy = 0usize;
        for (j, (a, b)) in prev.iter().zip(cur.iter()).enumerate() {
            if a != b && noisy.insert(j) {
                new_noisy += 1;
            }
        }
        if new_noisy > 0 {
            eprintln!(
                "  snapshot {snap_count}: +{new_noisy} noisy (total: {})",
                noisy.len()
            );
        }
        prev = cur;

        // Check if user pressed Enter (non-blocking on Windows)
        // We'll use a simple heuristic: run for at least 3 seconds,
        // then check. For simplicity, just run a fixed duration.
        if baseline_start.elapsed() >= Duration::from_secs(5) {
            break;
        }
    }

    eprintln!(
        "Baseline complete: {} noisy addresses from {snap_count} snapshots in {:.1}s",
        noisy.len(),
        baseline_start.elapsed().as_secs_f64()
    );
    eprintln!();

    // ================================================================
    // Phase 2+3 loop: Monitor then Confirm
    // ================================================================
    // Track surviving candidates across rounds for narrowing.
    let mut surviving_candidates: Option<HashSet<usize>> = None;

    loop {
        // --- Phase 2: Monitor ---
        wait_for_enter(
            "Press ENTER, then switch to the game and trigger the action.\n\
             Press ENTER again here when the screen has updated.\n> ",
        );

        eprintln!("Monitoring... switch to the game and perform the action.");
        eprintln!("Come back and press ENTER when done.");

        let anchor = snapshot(mem, dos_base); // "before" state
        // candidates: address -> (first_seen_value, anchor_value)
        // We track the value when we first saw it change and the anchor value.
        let mut candidates: HashMap<usize, (u8, u8)> = HashMap::new();
        let mut poll_count = 0u64;
        let monitor_start = Instant::now();

        // Monitor loop — read stdin line to stop
        // Use a simple approach: poll in a tight loop, periodically check
        // if enough time has passed and user might have pressed Enter.
        // For simplicity, we'll just loop until the user presses Enter.
        //
        // To detect Enter without blocking, we spawn a thread.
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let _input_thread = std::thread::spawn(move || {
            let mut line = String::new();
            io::stdin().read_line(&mut line).unwrap();
            let _ = stop_tx.send(());
        });

        let mut prev_snap = anchor.clone();
        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }

            thread::sleep(MONITOR_INTERVAL);
            let cur = snapshot(mem, dos_base);
            poll_count += 1;

            for (j, (old, new)) in prev_snap.iter().zip(cur.iter()).enumerate() {
                if old != new && !noisy.contains(&j) {
                    candidates.entry(j).or_insert((*old, anchor[j]));
                }
            }
            prev_snap = cur;
        }

        let monitor_elapsed = monitor_start.elapsed();
        eprintln!(
            "Monitor stopped: {poll_count} polls in {:.1}s, {} raw candidates",
            monitor_elapsed.as_secs_f64(),
            candidates.len()
        );

        // --- Phase 3: Confirm — which candidates returned to anchor value? ---
        let final_snap = snapshot(mem, dos_base);

        let mut returned: Vec<usize> = Vec::new();
        let mut stayed_changed: Vec<usize> = Vec::new();

        for (&addr, &(_first_val, anchor_val)) in &candidates {
            if final_snap[addr] == anchor_val {
                returned.push(addr);
            } else {
                stayed_changed.push(addr);
            }
        }

        returned.sort();
        stayed_changed.sort();

        eprintln!();
        if !returned.is_empty() {
            eprintln!(
                "=== RETURNED TO ORIGINAL ({} addresses) — likely dirty flags ===",
                returned.len()
            );
            for &addr in &returned {
                let (_first_val, anchor_val) = candidates[&addr];
                let current = final_snap[addr];
                eprintln!(
                    "  {}  anchor={:#04X}  now={:#04X}",
                    format_addr(addr, dos_base),
                    anchor_val,
                    current
                );
            }
        } else {
            eprintln!("=== No addresses returned to original value ===");
        }

        eprintln!();
        if !stayed_changed.is_empty() {
            eprintln!(
                "=== STAYED CHANGED ({} addresses) — state changes, not flags ===",
                stayed_changed.len()
            );
            for &addr in &stayed_changed {
                let (_first_val, anchor_val) = candidates[&addr];
                let current = final_snap[addr];
                eprintln!(
                    "  {}  anchor={:#04X}  now={:#04X}",
                    format_addr(addr, dos_base),
                    anchor_val,
                    current
                );
            }
        }

        // --- Narrow across rounds ---
        let returned_set: HashSet<usize> = returned.iter().copied().collect();
        match &mut surviving_candidates {
            None => {
                surviving_candidates = Some(returned_set);
            }
            Some(prev_survivors) => {
                let before_count = prev_survivors.len();
                prev_survivors.retain(|addr| returned_set.contains(addr));
                eprintln!();
                eprintln!(
                    "=== NARROWING: {} -> {} surviving candidates ===",
                    before_count,
                    prev_survivors.len()
                );
                if !prev_survivors.is_empty() {
                    let mut sorted: Vec<usize> = prev_survivors.iter().copied().collect();
                    sorted.sort();
                    for addr in &sorted {
                        eprintln!("  {}", format_addr(*addr, dos_base));
                    }
                }
            }
        }

        eprintln!();
        eprintln!("Ready for another round. Repeat to narrow further, or Ctrl+C to quit.");
    }
}
