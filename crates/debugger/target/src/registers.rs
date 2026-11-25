use alloc::vec::Vec;

/// Register values by DWARF number: those known, each up to 128 bits (a
/// general register's value is its low 64). Kept sorted and sparse: a
/// context has a few dozen registers, out of numbers up to 512.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registers {
    values: Vec<(u16, u128)>,
}

impl Registers {
    pub fn new() -> Registers {
        Registers::default()
    }

    /// Register `register`'s value, if known.
    pub fn get(&self, register: u16) -> Option<u128> {
        let i = self
            .values
            .binary_search_by_key(&register, |&(r, _)| r)
            .ok()?;
        Some(self.values[i].1)
    }

    /// Its low 64 bits, if known.
    pub fn get64(&self, register: u16) -> Option<u64> {
        // (Truncation is the point: a general register's value.)
        #[allow(clippy::cast_possible_truncation)]
        self.get(register).map(|v| v as u64)
    }

    /// Sets register `register`'s value, or forgets it (`None`).
    pub fn set(&mut self, register: u16, value: Option<u128>) {
        match (
            self.values.binary_search_by_key(&register, |&(r, _)| r),
            value,
        ) {
            (Ok(i), Some(v)) => self.values[i].1 = v,
            (Ok(i), None) => {
                self.values.remove(i);
            }
            (Err(i), Some(v)) => self.values.insert(i, (register, v)),
            (Err(_), None) => {}
        }
    }

    /// The known registers, in number order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, u128)> + '_ {
        self.values.iter().copied()
    }

    /// How many are known.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// As the unwinders take them: each register's low 64 bits (registers
    /// numbered past [`turbonop_unwind::dwarf_cfi::Registers::LIMIT`]
    /// can't be).
    pub fn to_unwind(&self) -> turbonop_unwind::dwarf_cfi::Registers {
        let mut r = turbonop_unwind::dwarf_cfi::Registers::default();
        for (n, v) in self.iter() {
            #[allow(clippy::cast_possible_truncation)]
            r.set(n, Some(v as u64));
        }
        r
    }

    /// From the unwinders' registers.
    pub fn from_unwind(registers: &turbonop_unwind::dwarf_cfi::Registers) -> Registers {
        Registers {
            values: registers.iter().map(|(n, v)| (n, u128::from(v))).collect(),
        }
    }
}
