//! Access data (what instructions read and write) against Intel XED, on an
//! encoding of every opcode table form (`tests/data/access.txt`, made by
//! `tools/x86access`).

use std::collections::BTreeSet;
use turbonop_x86::{Flags, Instruction, Mnemonic, Mode, RegClass, Register, decode, form_origin};

fn bytes(hex: &str) -> Vec<u8> {
    hex.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

/// A register as XED names it, or `None` for what the comparison leaves out.
fn xed_name(r: Register) -> Option<String> {
    let r = r.full();
    Some(match r.class() {
        RegClass::Ip => return None,
        RegClass::System => match r {
            Register::FPSW => "x87status".to_string(),
            Register::FPCW => "x87control".to_string(),
            Register::FPTW => "x87tag".to_string(),
            Register::MSR => "msrs".to_string(),
            Register::TILECFG => "tileconfig".to_string(),
            _ => r.name().to_string(),
        },
        _ => r.name().to_string(),
    })
}

/// XED's register names, mapped to ours (`None`: left out).
fn normalize_xed(name: &str) -> Option<&str> {
    if name.starts_with("tmp") || name.starts_with("dfv") || name.starts_with("x87last") {
        return None;
    }
    Some(match name {
        "" | "rip" | "eip" | "ip" | "rflags" | "eflags" | "flags" | "stackpush" | "stackpop"
        | "x87push" | "x87pop" | "x87pop2" | "x87opcode" | "error" | "bsr0" => return None,
        "fsbase" => "fs",
        "gsbase" => "gs",
        "tsc" | "tscaux" | "ia32_kernel_gs_base" | "ia32_u_cet" => "msrs",
        _ => name,
    })
}

fn is_segment(name: &str) -> bool {
    matches!(name, "es" | "cs" | "ss" | "ds" | "fs" | "gs")
}

/// All RFLAGS bits; the ones whose reads are compared (the status flags
/// and DF: system flags are read for privilege checks XED models and we
/// don't).
const ALL_FLAGS: u32 = 0x003F_7FD5;
const READ_FLAGS: u32 = Flags::STATUS.0 | Flags::DF.0;

#[derive(Default, Debug, PartialEq, Eq, Clone)]
struct Summary {
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
    mem_read: bool,
    mem_written: bool,
    flags_read: u32,
    flags_written: u32,
}

fn ours(inst: &Instruction) -> Summary {
    let mut s = Summary::default();
    for r in inst.read_registers().iter() {
        if let Some(n) = xed_name(r) {
            s.reads.insert(n);
        }
    }
    for r in inst.written_registers().iter() {
        if let Some(n) = xed_name(r) {
            s.writes.insert(n);
        }
    }
    s.mem_read = inst.reads_memory();
    s.mem_written = inst.writes_memory();
    let f = inst.flags();
    s.flags_read = f.read.0;
    s.flags_written = f.written.0;
    s
}

struct Golden {
    mode: Mode,
    hex: String,
    iclass: String,
    len: usize,
    xed: Summary,
}

fn golden() -> Vec<Golden> {
    let mut out = Vec::new();
    for line in include_str!("../data/access.txt").lines() {
        if line.starts_with('#') || line.is_empty() || line.contains("| error") {
            continue;
        }
        let parts: Vec<&str> = line.split(" | ").collect();
        let (mode, hex) = parts[0].split_once(' ').unwrap();
        let (iclass, len) = parts[1].split_once(' ').unwrap();
        let regs = |field: &str, prefix: &str| -> BTreeSet<String> {
            field
                .strip_prefix(prefix)
                .unwrap()
                .split(',')
                .filter_map(normalize_xed)
                .map(str::to_string)
                .collect()
        };
        let m = parts[4].strip_prefix("m=").unwrap();
        let f: Vec<u32> = parts[5]
            .strip_prefix("f=")
            .unwrap()
            .split('/')
            .map(|x| u32::from_str_radix(x, 16).unwrap())
            .collect();
        out.push(Golden {
            mode: if mode == "64" {
                Mode::Bits64
            } else {
                Mode::Bits32
            },
            hex: hex.to_string(),
            iclass: iclass.to_string(),
            len: len.parse().unwrap(),
            xed: Summary {
                reads: regs(parts[2], "r="),
                writes: regs(parts[3], "w="),
                mem_read: m.contains('r'),
                mem_written: m.contains('w'),
                flags_read: f[0],
                flags_written: f[1],
            },
        });
    }
    out
}

/// Forms where XED knows less than we do, or differs from the SDM, and
/// our data is the right one: (mnemonic, why).
const XED_DIFFERS: &[(&str, &str)] = &[(
    "nop",
    "XED reads the ModRM registers and memory of a multi-byte NOP",
)];

/// Instructions whose data knowingly has more than XED's, by mnemonic (or
/// prefix, ending in `*`) or XED iclass: (name, why).
const OVER_APPROXIMATE: &[(&str, &str)] = &[
    ("xsave*", "XED leaves out the state components saved"),
    ("xrstor*", "XED leaves out the state components restored"),
    ("fxsave*", "XED leaves out the state saved"),
    ("fxrstor*", "XED leaves out the state restored"),
    ("fnsave", "XED leaves out the x87 registers saved"),
    ("frstor", "XED leaves out the x87 state restored"),
    ("fldenv", "XED leaves out the control and tag words"),
    ("fnstenv", "storing the environment masks the exceptions"),
    ("fdisi", "the 8087's no-ops write the control word"),
    ("feni", "the 8087's no-ops write the control word"),
    ("fsetpm", "the 287's no-ops write the control word"),
    ("ffree*", "the status word (as every x87 instruction may)"),
    ("vmsave", "XED leaves out the state VMSAVE stores"),
    ("vmload", "XED leaves out the state VMLOAD loads"),
    ("vmrun", "the guest runs: every register may change"),
    ("syscall", "the kernel's results: every register may change"),
    (
        "sysenter",
        "the kernel's results: every register may change",
    ),
    ("int", "a system call's results: every register may change"),
    ("vmcall", "the hypervisor's results"),
    ("vmmcall", "the hypervisor's results"),
    ("vmgexit", "the hypervisor's results"),
    ("tdcall", "the TDX module's results"),
    ("seamcall", "the TDX module's results"),
    ("seamops", "the TDX module's results"),
    ("seamret", "the TDX module's results"),
    ("skinit", "a secure launch: every register may change"),
    ("rsm", "SMM's results: every register may change"),
    ("CALL_FAR", "XED leaves out the CS load"),
    ("JMP_FAR", "XED leaves out the CS load"),
    ("RET_FAR", "XED leaves out the CS (and SS) loads"),
    ("iret*", "XED leaves out the CS and SS loads"),
    ("sysexit", "XED leaves out the CS and SS loads"),
    (
        "sysret*",
        "XED leaves out the CS and SS loads, and the read of R11",
    ),
    ("erets", "FRED's return: CS, SS, GS, and MSR state"),
    ("eretu", "FRED's return: CS, SS, GS, and MSR state"),
    ("uiret", "XED leaves out the flags and UIF"),
    ("clts", "XED leaves out the write of CR0.TS"),
    (
        "vp2intersect*",
        "XED leaves out the second mask register of the pair",
    ),
    ("clzero", "XED leaves out the cache line written at [rAX]"),
    (
        "enqcmd*",
        "XED leaves out the 64 bytes written at the destination",
    ),
    ("swapgs", "the kernel GS base MSR"),
    ("rdmsrlist", "the MSRs, and RSI and RDI's bits"),
    ("wrmsrlist", "the MSRs, and RSI and RDI's bits"),
    ("vzero*", "XED lists no registers"),
    ("encodekey*", "Key Locker: XMM0-XMM7 as a group"),
    ("ldtilecfg", "the tiles are zeroed"),
    ("tilerelease", "the tiles are zeroed"),
    ("getsec", "leaf-dependent registers and flags"),
    ("encl*", "leaf-dependent registers and flags"),
    ("rmp*", "leaf-dependent registers and flags"),
    ("psmash", "the status flags"),
    ("setssbsy", "the SDM's flags, and the shadow stack pointer"),
    ("clrssbsy", "the SDM's flags"),
    ("rstorssp", "the SDM's flags"),
    ("pbndkb", "the SDM's flags"),
    ("rdpru", "the SDM's flags"),
    ("testui", "the SDM's flags"),
    ("vptest", "the SDM's flags (XED has PTEST's)"),
    ("vtestp*", "the SDM's flags"),
    ("ccmp*", "XED leaves out the source condition's flags"),
    ("ctest*", "XED leaves out the source condition's flags"),
    ("rdpkru", "XED leaves out PKRU"),
    ("wrpkru", "XED leaves out PKRU"),
    ("mwaitx", "XED leaves out the timeout in rBX"),
    ("vmfunc", "XED leaves out the EPTP index in ECX"),
    ("lwpins", "XED has the data register written only"),
    ("lwpval", "XED has the data register written only"),
    ("llwpcb", "XED has the LWPCB address written only"),
    ("extrq", "the SDM's EXTRQ reads its destination"),
    ("insertq", "the SDM's INSERTQ reads its destination"),
    ("movss", "a register source merges, a memory one doesn't"),
    ("movsd", "a register source merges, a memory one doesn't"),
];

fn listed(list: &[(&str, &str)], name: &str, iclass: &str) -> bool {
    list.iter().any(|&(m, _)| match m.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => m == name || m == iclass,
    })
}

#[test]
fn agrees_with_xed() {
    let cases = golden();
    assert!(cases.len() > 5000, "{} cases", cases.len());
    let mut missing = Vec::new();
    let mut extra = Vec::new();
    let mut undecoded = Vec::new();
    let mut hint_nops = 0;
    for c in &cases {
        let b = bytes(&c.hex);
        let inst = match decode(&b, c.mode, 0x1000) {
            Ok(i) if i.len() == c.len => i,
            _ => {
                undecoded.push(format!("{} ({})", c.hex, c.iclass));
                continue;
            }
        };
        let name = inst.mnemonic().name();
        if listed(XED_DIFFERS, name, &c.iclass) {
            continue;
        }
        // XED decodes the hint space (MPX, CET, ...) as NOPs.
        if c.iclass == "NOP" && inst.mnemonic() != Mnemonic::Nop {
            hint_nops += 1;
            continue;
        }
        let o = ours(&inst);
        let x = &c.xed;
        let mut miss = Vec::new();
        let mut more = Vec::new();
        for r in x.writes.difference(&o.writes) {
            miss.push(format!("w:{r}"));
        }
        // XED reads k0 for an unmasked EVEX instruction: no mask.
        let unmasked = |r: &&String| inst.mask().is_none() && r.as_str() == "k0";
        for r in x
            .reads
            .difference(&o.reads)
            .filter(|r| !is_segment(r) && !unmasked(r))
        {
            miss.push(format!("r:{r}"));
        }
        // Where our model is knowingly wider than XED's, for every
        // instruction: an x87 push or pop renames every st(i) (XED has
        // st(0) written); st(i) and the MMX registers alias; and x87
        // instructions read the status and tag words (the top of the stack).
        // (x87 pushes and pops write the tag word, calls and returns the
        // shadow stack pointer, and AMX instructions read the tile
        // configuration, which XED leaves out.)
        let aliasing = |r: &&String| {
            r.starts_with("st(") || r.starts_with("mm") || matches!(r.as_str(), "x87tag" | "ssp")
        };
        for r in o.writes.difference(&x.writes).filter(|r| !aliasing(r)) {
            more.push(format!("w:{r}"));
        }
        let x87_state =
            |r: &&String| matches!(r.as_str(), "x87status" | "x87tag" | "tileconfig" | "ssp");
        for r in o
            .reads
            .difference(&x.reads)
            .filter(|r| !is_segment(r) && !x87_state(r))
        {
            more.push(format!("r:{r}"));
        }
        if x.mem_written && !o.mem_written {
            miss.push("w:mem".to_string());
        }
        if x.mem_read && !o.mem_read {
            miss.push("r:mem".to_string());
        }
        if o.mem_written && !x.mem_written {
            more.push("w:mem".to_string());
        }
        if o.mem_read && !x.mem_read {
            more.push("r:mem".to_string());
        }
        let fw = x.flags_written & ALL_FLAGS;
        if fw & !o.flags_written != 0 {
            miss.push(format!("wf:{:?}", Flags(fw & !o.flags_written)));
        }
        if o.flags_written & !fw != 0 {
            more.push(format!("wf:{:?}", Flags(o.flags_written & !fw)));
        }
        let fr = x.flags_read & READ_FLAGS;
        if fr & !o.flags_read != 0 {
            miss.push(format!("rf:{:?}", Flags(fr & !o.flags_read)));
        }
        if o.flags_read & READ_FLAGS & !fr != 0 {
            more.push(format!("rf:{:?}", Flags(o.flags_read & READ_FLAGS & !fr)));
        }
        let what = || {
            format!(
                "{} {} [{} {}]",
                c.hex,
                turbonop_x86::format::to_string(&inst),
                c.iclass,
                form_origin(inst.form()).unwrap_or("?")
            )
        };
        if !miss.is_empty() {
            missing.push(format!("{}: missing {}", what(), miss.join(" ")));
        }
        if !more.is_empty() && !listed(OVER_APPROXIMATE, name, &c.iclass) {
            extra.push(format!("{}: extra {}", what(), more.join(" ")));
        }
    }
    let show = |v: &[String]| v[..v.len().min(5000)].join("\n");
    assert!(
        missing.is_empty() && extra.is_empty() && undecoded.is_empty(),
        "{} of {} cases miss accesses XED has:\n{}\n\n{} have extra ones:\n{}\n\n\
         {} decode differently from XED: {undecoded:?}",
        missing.len(),
        cases.len(),
        show(&missing),
        extra.len(),
        show(&extra),
        undecoded.len(),
    );
    // (XED decodes some of the hint space as NOPs: MPX, CET.)
    assert!(hint_nops < 40, "{hint_nops} hint NOPs");
}

/// Prints what each of `ACCESS_HEX`'s instructions (`;`-separated hex, 64-bit
/// mode) reads and writes.
#[test]
#[ignore]
fn show() {
    let input = std::env::var("ACCESS_HEX").unwrap_or_default();
    for hex in input.split(';').filter(|h| !h.trim().is_empty()) {
        let Ok(inst) = decode(&bytes(hex), Mode::Bits64, 0x1000) else {
            println!("{hex}: invalid");
            continue;
        };
        println!(
            "{hex}: {} [{}]",
            turbonop_x86::format::to_string(&inst),
            form_origin(inst.form()).unwrap_or("?")
        );
        for a in inst.accesses() {
            println!(
                "    {:?} {:?}{}",
                a.rw,
                a.operand,
                if a.implicit { " (implicit)" } else { "" }
            );
        }
        println!("    flags {:?}", inst.flags());
        println!("    reads {:?}", inst.read_registers());
        println!("    writes {:?}", inst.written_registers());
    }
}
