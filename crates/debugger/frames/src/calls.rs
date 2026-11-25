//! Whether an address is a return address: whether the instruction just
//! before it is a call that returns there.

use turbonop_code::{Flow, Mode, decode_in};
use turbonop_core::Arch;
use turbonop_target::AddressSpace;

/// The most bytes an x86 call takes (`call qword ptr [rip+disp32]` with
/// prefixes; `call [base+index*scale+disp32]`).
const X86_LONGEST_CALL: u64 = 8;
const X86_LONGEST_CALL_BYTES: usize = 8;

/// Whether the instruction ending at `address` is a call (so `address` is
/// where it returns), by the code `space` has there. False if the code
/// isn't available.
pub fn follows_call(space: &AddressSpace<'_>, arch: Arch, address: u64) -> bool {
    match arch {
        Arch::X64 | Arch::X86 => {
            let Some(start) = address.checked_sub(X86_LONGEST_CALL) else {
                return false;
            };
            let mut code = [0u8; X86_LONGEST_CALL_BYTES];
            if space.read(start, &mut code).is_none() {
                // (Fewer bytes, near a page the dump didn't capture.)
                let mut short = [0u8; 2];
                return space.read(address - 2, &mut short).is_some()
                    && is_call(arch, &short, address - 2, address);
            }
            (2..=X86_LONGEST_CALL).any(|back| {
                let at = address - back;
                let i = usize::try_from(X86_LONGEST_CALL - back).unwrap_or(0);
                is_call(arch, &code[i..], at, address)
            })
        }
        Arch::Arm64 => {
            let mut word = [0u8; 4];
            address.checked_sub(4).is_some_and(|at| {
                space.read(at, &mut word).is_some() && is_call(arch, &word, at, address)
            })
        }
        Arch::Arm => {
            // A return address keeps its Thumb bit: Thumb calls are 2 or 4
            // bytes (BL, BLX), A32 calls 4.
            let thumb = address & 1 != 0;
            let address = address & !1;
            let mut code = [0u8; 4];
            let Some(at) = address.checked_sub(4) else {
                return false;
            };
            if space.read(at, &mut code).is_none() {
                return false;
            }
            // (Thumb code is decoded at its address with bit 0 set.)
            let ok = |bytes: &[u8], at: u64| {
                let as_decoded = if thumb { at | 1 } else { at };
                decode_in(arch, Mode::default(), bytes, as_decoded)
                    .is_some_and(|i| i.flow == Flow::Call && at + u64::from(i.len) == address)
            };
            ok(&code, at) || (thumb && ok(&code[2..], at + 2))
        }
        _ => false,
    }
}

fn is_call(arch: Arch, bytes: &[u8], at: u64, returns_to: u64) -> bool {
    decode_in(arch, Mode::default(), bytes, at)
        .is_some_and(|i| i.flow == Flow::Call && at + u64::from(i.len) == returns_to)
}
