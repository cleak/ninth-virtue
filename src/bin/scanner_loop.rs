//! Repeatedly reads game state from DOSBox to validate scanner stability.

use std::thread;
use std::time::Duration;

fn main() {
    use u5_companion::game::character::read_party;
    use u5_companion::game::inventory::read_inventory;
    use u5_companion::memory::process;
    use u5_companion::memory::scanner;

    println!("=== Scanner Stability Loop ===\n");

    let procs = process::list_dosbox_processes().unwrap();
    if procs.is_empty() {
        eprintln!("No DOSBox process found");
        return;
    }
    let (pid, name) = &procs[0];
    println!("Attaching to {name} (PID {pid})...");

    let proc = process::attach(*pid).unwrap();
    let result = scanner::find_dos_base(&proc.memory).unwrap();
    println!(
        "DOS base: {:#x} (confirmed={})\n",
        result.dos_base, result.game_confirmed
    );

    let dos_base = result.dos_base;

    for i in 1..=10 {
        println!("--- Read #{i} ---");

        match read_party(&proc.memory, dos_base) {
            Ok(party) => {
                for ch in &party {
                    print!(
                        "  {} HP={}/{} {:?} | ",
                        ch.name, ch.hp, ch.max_hp, ch.status
                    );
                }
                println!();
            }
            Err(e) => {
                eprintln!("  Party read FAILED: {e}");
                return;
            }
        }

        match read_inventory(&proc.memory, dos_base) {
            Ok(inv) => {
                println!(
                    "  F={} G={} K={} Ge={} T={} Ar={} Ka={}",
                    inv.food, inv.gold, inv.keys, inv.gems, inv.torches, inv.arrows, inv.karma
                );
            }
            Err(e) => {
                eprintln!("  Inventory read FAILED: {e}");
                return;
            }
        }

        thread::sleep(Duration::from_secs(2));
    }

    println!("\nAll 10 reads succeeded — scanner is stable.");
}
