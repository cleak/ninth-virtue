use anyhow::Result;

use crate::game::offsets::*;
use crate::memory::access::MemoryAccess;

/// The eight virtues of Ultima V, in bit-index order within the shrine
/// quest bitmasks at save offsets 0x326 (ordained) and 0x328 (codex visited).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Virtue {
    Honesty = 0,
    Compassion = 1,
    Valor = 2,
    Justice = 3,
    Sacrifice = 4,
    Honor = 5,
    Spirituality = 6,
    Humility = 7,
}

impl Virtue {
    pub const ALL: [Virtue; 8] = [
        Virtue::Honesty,
        Virtue::Compassion,
        Virtue::Valor,
        Virtue::Justice,
        Virtue::Sacrifice,
        Virtue::Honor,
        Virtue::Spirituality,
        Virtue::Humility,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Virtue::Honesty => "Honesty",
            Virtue::Compassion => "Compassion",
            Virtue::Valor => "Valor",
            Virtue::Justice => "Justice",
            Virtue::Sacrifice => "Sacrifice",
            Virtue::Honor => "Honor",
            Virtue::Spirituality => "Spirituality",
            Virtue::Humility => "Humility",
        }
    }

    pub fn mantra(self) -> &'static str {
        match self {
            Virtue::Honesty => "AHM",
            Virtue::Compassion => "MU",
            Virtue::Valor => "RA",
            Virtue::Justice => "BEH",
            Virtue::Sacrifice => "CAH",
            Virtue::Honor => "SUMM",
            Virtue::Spirituality => "OM",
            Virtue::Humility => "LUM",
        }
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// Phase of the shrine quest for a single virtue.
///
/// The game tracks two independent bits per virtue:
/// - **ordained** (0x326): set when the player meditates at the shrine,
///   cleared when the player returns to the shrine after visiting the Codex.
/// - **codex** (0x328): set when the player visits the Codex for this
///   virtue, stays set permanently.
///
/// The four states form a progression:
///   NotStarted → Ordained → CodexRead → Complete
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestPhase {
    /// Neither bit set — player has not meditated at this shrine.
    NotStarted,
    /// Ordained=1, Codex=0 — player meditated at the shrine, needs to
    /// visit the Codex.
    Ordained,
    /// Ordained=1, Codex=1 — player visited the Codex, needs to return
    /// to the shrine to complete the quest.
    CodexRead,
    /// Ordained=0, Codex=1 — quest complete. The ordained bit is cleared
    /// when the player returns to the shrine.
    Complete,
}

/// Shrine quest progress for all eight virtues, read from memory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShrineQuest {
    /// Bitmask at save offset 0x326: bit N set = ordained at shrine N.
    /// Cleared when the quest is turned in at the shrine.
    pub ordained: u8,
    /// Bitmask at save offset 0x328: bit N set = visited Codex for virtue N.
    /// Stays set permanently once the Codex is visited.
    pub codex: u8,
}

impl ShrineQuest {
    pub fn phase(&self, virtue: Virtue) -> QuestPhase {
        let bit = virtue.bit();
        let is_ordained = self.ordained & bit != 0;
        let is_codex = self.codex & bit != 0;
        match (is_ordained, is_codex) {
            (false, false) => QuestPhase::NotStarted,
            (true, false) => QuestPhase::Ordained,
            (true, true) => QuestPhase::CodexRead,
            (false, true) => QuestPhase::Complete,
        }
    }
}

pub fn read_shrine_quest(mem: &dyn MemoryAccess, dos_base: usize) -> Result<ShrineQuest> {
    let ordained = mem.read_u8(inv_addr(dos_base, SHRINE_ORDAINED))?;
    let codex = mem.read_u8(inv_addr(dos_base, SHRINE_CODEX_VISITED))?;
    Ok(ShrineQuest { ordained, codex })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::access::MockMemory;

    fn mock_with_quest(ordained: u8, codex: u8) -> (MockMemory, usize) {
        let dos_base = 0x1000;
        let size = SAVE_BASE + SHRINE_CODEX_VISITED + 1;
        let mem = MockMemory::new(dos_base + size);
        mem.write_u8(inv_addr(dos_base, SHRINE_ORDAINED), ordained)
            .unwrap();
        mem.write_u8(inv_addr(dos_base, SHRINE_CODEX_VISITED), codex)
            .unwrap();
        (mem, dos_base)
    }

    #[test]
    fn all_not_started() {
        let (mem, dos_base) = mock_with_quest(0x00, 0x00);
        let q = read_shrine_quest(&mem, dos_base).unwrap();
        for v in Virtue::ALL {
            assert_eq!(q.phase(v), QuestPhase::NotStarted);
        }
    }

    #[test]
    fn ordained_only() {
        // Ordained at Honesty (bit 0) and Valor (bit 2)
        let (mem, dos_base) = mock_with_quest(0x05, 0x00);
        let q = read_shrine_quest(&mem, dos_base).unwrap();
        assert_eq!(q.phase(Virtue::Honesty), QuestPhase::Ordained);
        assert_eq!(q.phase(Virtue::Compassion), QuestPhase::NotStarted);
        assert_eq!(q.phase(Virtue::Valor), QuestPhase::Ordained);
    }

    #[test]
    fn codex_read() {
        // Ordained and codex visited for Honesty — needs to return to shrine
        let (mem, dos_base) = mock_with_quest(0x01, 0x01);
        let q = read_shrine_quest(&mem, dos_base).unwrap();
        assert_eq!(q.phase(Virtue::Honesty), QuestPhase::CodexRead);
    }

    #[test]
    fn complete() {
        // Codex visited but ordained cleared — quest turned in
        let (mem, dos_base) = mock_with_quest(0x00, 0x01);
        let q = read_shrine_quest(&mem, dos_base).unwrap();
        assert_eq!(q.phase(Virtue::Honesty), QuestPhase::Complete);
    }

    #[test]
    fn mixed_states() {
        // Matches the user's live game state: ordained=0x39, codex=0x01
        // 0x39 = bits 0,3,4,5 = Honesty, Justice, Sacrifice, Honor
        // 0x01 = bit 0 = Honesty
        let (mem, dos_base) = mock_with_quest(0x39, 0x01);
        let q = read_shrine_quest(&mem, dos_base).unwrap();
        assert_eq!(q.phase(Virtue::Honesty), QuestPhase::CodexRead); // both bits
        assert_eq!(q.phase(Virtue::Compassion), QuestPhase::NotStarted);
        assert_eq!(q.phase(Virtue::Valor), QuestPhase::NotStarted);
        assert_eq!(q.phase(Virtue::Justice), QuestPhase::Ordained); // ordained only
        assert_eq!(q.phase(Virtue::Sacrifice), QuestPhase::Ordained);
        assert_eq!(q.phase(Virtue::Honor), QuestPhase::Ordained);
        assert_eq!(q.phase(Virtue::Spirituality), QuestPhase::NotStarted);
        assert_eq!(q.phase(Virtue::Humility), QuestPhase::NotStarted);
    }

    #[test]
    fn all_complete() {
        // All codex visited, no ordained bits — all quests turned in
        let (mem, dos_base) = mock_with_quest(0x00, 0xFF);
        let q = read_shrine_quest(&mem, dos_base).unwrap();
        for v in Virtue::ALL {
            assert_eq!(q.phase(v), QuestPhase::Complete);
        }
    }

    #[test]
    fn virtue_names_and_mantras() {
        assert_eq!(Virtue::Honesty.name(), "Honesty");
        assert_eq!(Virtue::Honesty.mantra(), "AHM");
        assert_eq!(Virtue::Humility.name(), "Humility");
        assert_eq!(Virtue::Humility.mantra(), "LUM");
    }
}
