//! CLI tool to test the DOS memory scanner against a live DOSBox process.

fn main() {
    use ninth_virtue::game::character::read_party;
    use ninth_virtue::game::inventory::read_inventory;
    use ninth_virtue::memory::process;
    use ninth_virtue::memory::scanner;

    println!("=== DOSBox Memory Scanner Test ===\n");

    let procs = match process::list_dosbox_processes() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Process scan failed: {e}");
            return;
        }
    };
    if procs.is_empty() {
        eprintln!("No DOSBox process found");
        return;
    }

    let (pid, name) = &procs[0];
    println!("Found: {name} (PID {pid})");

    let proc = match process::attach(*pid) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Attach failed: {e}");
            return;
        }
    };

    println!("Running scanner...");
    let result = match scanner::find_dos_base(&proc.memory) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Scanner failed: {e}");
            return;
        }
    };

    println!(
        "dos_base = {:#x} (game_confirmed = {})",
        result.dos_base, result.game_confirmed
    );

    if !result.game_confirmed {
        println!("Game not loaded — load a save and try again");
        return;
    }

    println!("\n--- Party ---");
    match read_party(&proc.memory, result.dos_base) {
        Ok(party) => {
            for ch in &party {
                println!(
                    "  {} ({:?}) {:?} HP={}/{} STR={} DEX={} INT={} MP={} XP={} Lvl={}",
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
        Err(e) => eprintln!("  Read failed: {e}"),
    }

    println!("\n--- Inventory ---");
    match read_inventory(&proc.memory, result.dos_base) {
        Ok(inv) => {
            println!(
                "  Food={} Gold={} Keys={} Gems={}",
                inv.food, inv.gold, inv.keys, inv.gems
            );
            println!(
                "  Torches={} Arrows={} Karma={}",
                inv.torches, inv.arrows, inv.karma
            );
            println!("  Reagents: {:?}", inv.reagents);
        }
        Err(e) => eprintln!("  Read failed: {e}"),
    }
}
