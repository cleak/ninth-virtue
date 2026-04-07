use anyhow::Result;

use crate::game::offsets::*;
use crate::memory::access::MemoryAccess;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
}

impl TryFrom<u8> for Gender {
    type Error = anyhow::Error;
    fn try_from(v: u8) -> Result<Self> {
        match v {
            0x0B => Ok(Gender::Male),
            0x0C => Ok(Gender::Female),
            _ => anyhow::bail!("invalid gender byte: {v:#x}"),
        }
    }
}

impl From<Gender> for u8 {
    fn from(g: Gender) -> u8 {
        match g {
            Gender::Male => 0x0B,
            Gender::Female => 0x0C,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    Avatar,
    Bard,
    Fighter,
    Mage,
}

impl CharClass {
    pub fn label(&self) -> &'static str {
        match self {
            CharClass::Avatar => "Avatar",
            CharClass::Bard => "Bard",
            CharClass::Fighter => "Fighter",
            CharClass::Mage => "Mage",
        }
    }
}

impl TryFrom<u8> for CharClass {
    type Error = anyhow::Error;
    fn try_from(v: u8) -> Result<Self> {
        match v {
            b'A' => Ok(CharClass::Avatar),
            b'B' => Ok(CharClass::Bard),
            b'F' => Ok(CharClass::Fighter),
            b'M' => Ok(CharClass::Mage),
            _ => anyhow::bail!("invalid class byte: {v:#x}"),
        }
    }
}

impl From<CharClass> for u8 {
    fn from(c: CharClass) -> u8 {
        match c {
            CharClass::Avatar => b'A',
            CharClass::Bard => b'B',
            CharClass::Fighter => b'F',
            CharClass::Mage => b'M',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Good,
    Poisoned,
    Asleep,
    Dead,
}

impl Status {
    pub const ALL: &[Status] = &[Status::Good, Status::Poisoned, Status::Asleep, Status::Dead];

    pub fn label(&self) -> &'static str {
        match self {
            Status::Good => "Good",
            Status::Poisoned => "Poisoned",
            Status::Asleep => "Asleep",
            Status::Dead => "Dead",
        }
    }
}

impl TryFrom<u8> for Status {
    type Error = anyhow::Error;
    fn try_from(v: u8) -> Result<Self> {
        match v {
            b'G' => Ok(Status::Good),
            b'P' => Ok(Status::Poisoned),
            b'S' => Ok(Status::Asleep),
            b'D' => Ok(Status::Dead),
            _ => anyhow::bail!("invalid status byte: {v:#x}"),
        }
    }
}

impl From<Status> for u8 {
    fn from(s: Status) -> u8 {
        match s {
            Status::Good => b'G',
            Status::Poisoned => b'P',
            Status::Asleep => b'S',
            Status::Dead => b'D',
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Character {
    pub index: usize,
    pub name: String,
    pub gender: Gender,
    pub class: CharClass,
    pub status: Status,
    pub str_: u8,
    pub dex: u8,
    pub int: u8,
    pub mp: u8,
    pub hp: u16,
    pub max_hp: u16,
    pub xp: u16,
    pub level: u8,
    pub equipment: [u8; 6],
}

pub fn read_character(mem: &dyn MemoryAccess, dos_base: usize, index: usize) -> Result<Character> {
    let addr = |field: usize| char_addr(dos_base, index, field);

    let mut name_buf = [0u8; CHAR_NAME_LEN];
    mem.read_bytes(addr(CHAR_NAME), &mut name_buf)?;
    let null_pos = name_buf
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(CHAR_NAME_LEN);
    let name = String::from_utf8_lossy(&name_buf[..null_pos]).to_string();

    let gender = Gender::try_from(mem.read_u8(addr(CHAR_GENDER))?)?;
    let class = CharClass::try_from(mem.read_u8(addr(CHAR_CLASS))?)?;
    let status = Status::try_from(mem.read_u8(addr(CHAR_STATUS))?)?;
    let str_ = mem.read_u8(addr(CHAR_STR))?;
    let dex = mem.read_u8(addr(CHAR_DEX))?;
    let int = mem.read_u8(addr(CHAR_INT))?;
    let mp = mem.read_u8(addr(CHAR_MP))?;
    let hp = mem.read_u16_le(addr(CHAR_HP))?;
    let max_hp = mem.read_u16_le(addr(CHAR_MAX_HP))?;
    let xp = mem.read_u16_le(addr(CHAR_XP))?;
    let level = mem.read_u8(addr(CHAR_LEVEL))?;

    let mut equipment = [0u8; CHAR_EQUIPMENT_LEN];
    mem.read_bytes(addr(CHAR_EQUIPMENT), &mut equipment)?;

    Ok(Character {
        index,
        name,
        gender,
        class,
        status,
        str_,
        dex,
        int,
        mp,
        hp,
        max_hp,
        xp,
        level,
        equipment,
    })
}

pub fn write_character(mem: &dyn MemoryAccess, dos_base: usize, ch: &Character) -> Result<()> {
    let addr = |field: usize| char_addr(dos_base, ch.index, field);

    let mut name_buf = [0u8; CHAR_NAME_LEN];
    let name_bytes = ch.name.as_bytes();
    let len = name_bytes.len().min(CHAR_NAME_LEN - 1);
    name_buf[..len].copy_from_slice(&name_bytes[..len]);
    mem.write_bytes(addr(CHAR_NAME), &name_buf)?;

    mem.write_u8(addr(CHAR_GENDER), ch.gender.into())?;
    mem.write_u8(addr(CHAR_CLASS), ch.class.into())?;
    mem.write_u8(addr(CHAR_STATUS), ch.status.into())?;
    mem.write_u8(addr(CHAR_STR), ch.str_)?;
    mem.write_u8(addr(CHAR_DEX), ch.dex)?;
    mem.write_u8(addr(CHAR_INT), ch.int)?;
    mem.write_u8(addr(CHAR_MP), ch.mp)?;
    mem.write_u16_le(addr(CHAR_HP), ch.hp)?;
    mem.write_u16_le(addr(CHAR_MAX_HP), ch.max_hp)?;
    mem.write_u16_le(addr(CHAR_XP), ch.xp)?;
    mem.write_u8(addr(CHAR_LEVEL), ch.level)?;
    mem.write_bytes(addr(CHAR_EQUIPMENT), &ch.equipment)?;

    Ok(())
}

pub fn read_party(mem: &dyn MemoryAccess, dos_base: usize) -> Result<Vec<Character>> {
    let party_size = mem.read_u8(inv_addr(dos_base, INV_PARTY_SIZE))? as usize;
    let party_size = party_size.clamp(1, 6);
    let mut party = Vec::with_capacity(party_size);
    for i in 0..party_size {
        party.push(read_character(mem, dos_base, i)?);
    }
    Ok(party)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::access::MockMemory;

    fn setup_mock(party_size: u8) -> MockMemory {
        let total_size = SAVE_BASE + 0x300;
        let mem = MockMemory::new(total_size);

        mem.set_bytes(inv_addr(0, INV_PARTY_SIZE), &[party_size]);

        // Character 0: Avatar
        let c0 = char_addr(0, 0, 0);
        mem.set_bytes(c0 + CHAR_NAME, b"Avatar\0\0\0");
        mem.set_bytes(c0 + CHAR_GENDER, &[0x0B]);
        mem.set_bytes(c0 + CHAR_CLASS, b"A");
        mem.set_bytes(c0 + CHAR_STATUS, b"G");
        mem.set_bytes(c0 + CHAR_STR, &[30]);
        mem.set_bytes(c0 + CHAR_DEX, &[25]);
        mem.set_bytes(c0 + CHAR_INT, &[28]);
        mem.set_bytes(c0 + CHAR_MP, &[50]);
        mem.set_bytes(c0 + CHAR_HP, &200u16.to_le_bytes());
        mem.set_bytes(c0 + CHAR_MAX_HP, &240u16.to_le_bytes());
        mem.set_bytes(c0 + CHAR_XP, &5000u16.to_le_bytes());
        mem.set_bytes(c0 + CHAR_LEVEL, &[8]);
        mem.set_bytes(c0 + CHAR_EQUIPMENT, &[0x01, 0x05, 0x0A, 0xFF, 0xFF, 0xFF]);

        // Character 1: Shamino
        let c1 = char_addr(0, 1, 0);
        mem.set_bytes(c1 + CHAR_NAME, b"Shamino\0\0");
        mem.set_bytes(c1 + CHAR_GENDER, &[0x0B]);
        mem.set_bytes(c1 + CHAR_CLASS, b"F");
        mem.set_bytes(c1 + CHAR_STATUS, b"P");
        mem.set_bytes(c1 + CHAR_STR, &[25]);
        mem.set_bytes(c1 + CHAR_DEX, &[30]);
        mem.set_bytes(c1 + CHAR_INT, &[20]);
        mem.set_bytes(c1 + CHAR_MP, &[15]);
        mem.set_bytes(c1 + CHAR_HP, &100u16.to_le_bytes());
        mem.set_bytes(c1 + CHAR_MAX_HP, &180u16.to_le_bytes());
        mem.set_bytes(c1 + CHAR_XP, &3000u16.to_le_bytes());
        mem.set_bytes(c1 + CHAR_LEVEL, &[6]);
        mem.set_bytes(c1 + CHAR_EQUIPMENT, &[0x02, 0x06, 0xFF, 0xFF, 0xFF, 0xFF]);

        mem
    }

    #[test]
    fn read_character_fields() {
        let mem = setup_mock(2);
        let ch = read_character(&mem, 0, 0).unwrap();
        assert_eq!(ch.name, "Avatar");
        assert_eq!(ch.gender, Gender::Male);
        assert_eq!(ch.class, CharClass::Avatar);
        assert_eq!(ch.status, Status::Good);
        assert_eq!(ch.str_, 30);
        assert_eq!(ch.dex, 25);
        assert_eq!(ch.int, 28);
        assert_eq!(ch.mp, 50);
        assert_eq!(ch.hp, 200);
        assert_eq!(ch.max_hp, 240);
        assert_eq!(ch.xp, 5000);
        assert_eq!(ch.level, 8);
        assert_eq!(ch.equipment, [0x01, 0x05, 0x0A, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn read_poisoned_character() {
        let mem = setup_mock(2);
        let ch = read_character(&mem, 0, 1).unwrap();
        assert_eq!(ch.name, "Shamino");
        assert_eq!(ch.status, Status::Poisoned);
        assert_eq!(ch.class, CharClass::Fighter);
    }

    #[test]
    fn write_then_read_roundtrip() {
        let mem = setup_mock(2);
        let mut ch = read_character(&mem, 0, 0).unwrap();
        ch.hp = 150;
        ch.status = Status::Poisoned;
        ch.str_ = 45;
        write_character(&mem, 0, &ch).unwrap();
        let ch2 = read_character(&mem, 0, 0).unwrap();
        assert_eq!(ch, ch2);
    }

    #[test]
    fn read_party_respects_size() {
        let mem = setup_mock(2);
        let party = read_party(&mem, 0).unwrap();
        assert_eq!(party.len(), 2);
        assert_eq!(party[0].name, "Avatar");
        assert_eq!(party[1].name, "Shamino");
    }

    #[test]
    fn invalid_status_returns_error() {
        let mem = setup_mock(1);
        mem.set_bytes(char_addr(0, 0, CHAR_STATUS), b"X");
        assert!(read_character(&mem, 0, 0).is_err());
    }

    #[test]
    fn enum_roundtrips() {
        assert_eq!(Gender::try_from(0x0Bu8).unwrap(), Gender::Male);
        assert_eq!(Gender::try_from(0x0Cu8).unwrap(), Gender::Female);
        assert_eq!(u8::from(Gender::Male), 0x0B);
        assert_eq!(u8::from(Gender::Female), 0x0C);

        assert_eq!(CharClass::try_from(b'A').unwrap(), CharClass::Avatar);
        assert_eq!(CharClass::try_from(b'B').unwrap(), CharClass::Bard);
        assert_eq!(CharClass::try_from(b'F').unwrap(), CharClass::Fighter);
        assert_eq!(CharClass::try_from(b'M').unwrap(), CharClass::Mage);

        assert_eq!(Status::try_from(b'G').unwrap(), Status::Good);
        assert_eq!(Status::try_from(b'P').unwrap(), Status::Poisoned);
        assert_eq!(Status::try_from(b'S').unwrap(), Status::Asleep);
        assert_eq!(Status::try_from(b'D').unwrap(), Status::Dead);
    }

    #[test]
    fn short_name() {
        let mem = setup_mock(1);
        mem.set_bytes(char_addr(0, 0, CHAR_NAME), b"Jo\0\0\0\0\0\0\0");
        let ch = read_character(&mem, 0, 0).unwrap();
        assert_eq!(ch.name, "Jo");
    }

    #[test]
    fn max_length_name() {
        let mem = setup_mock(1);
        mem.set_bytes(char_addr(0, 0, CHAR_NAME), b"Abcdefgh\0");
        let ch = read_character(&mem, 0, 0).unwrap();
        assert_eq!(ch.name, "Abcdefgh");
    }
}
