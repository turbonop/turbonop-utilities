use super::*;

/// A `SYSTEM_PROCESS_INFORMATION` list, as Windows lays it out: entries,
/// each with its threads, and names after them all.
struct Builder {
    entries: Vec<(Raw, Vec<Thread>)>,
}

/// A thread: its context switches, state, and wait reason.
type Thread = (u32, u32, u32);

impl Builder {
    fn build(&self, base: u64) -> Vec<u8> {
        let sizes: Vec<usize> = self
            .entries
            .iter()
            .map(|(_, t)| PROCESS_SIZE + t.len() * THREAD_SIZE)
            .collect();
        let total: usize = sizes.iter().sum();
        let mut buf = vec![0u8; total];
        let mut names = Vec::new();
        let mut at = 0;
        for (i, (p, threads)) in self.entries.iter().enumerate() {
            let e = &mut buf[at..at + sizes[i]];
            let next = if i + 1 == self.entries.len() {
                0
            } else {
                sizes[i]
            };
            e[0..4].copy_from_slice(&u32::try_from(next).unwrap().to_ne_bytes());
            e[4..8].copy_from_slice(&u32::try_from(threads.len()).unwrap().to_ne_bytes());
            e[24..32].copy_from_slice(&p.cycles.to_ne_bytes());
            e[32..40].copy_from_slice(&p.create.to_ne_bytes());
            e[40..48].copy_from_slice(&p.user.to_ne_bytes());
            e[48..56].copy_from_slice(&p.kernel.to_ne_bytes());
            let name: Vec<u8> = p.name.encode_utf16().flat_map(u16::to_ne_bytes).collect();
            e[56..58].copy_from_slice(&u16::try_from(name.len()).unwrap().to_ne_bytes());
            e[58..60].copy_from_slice(&u16::try_from(name.len()).unwrap().to_ne_bytes());
            let ptr = if name.is_empty() {
                0
            } else {
                base + u64::try_from(total + names.len()).unwrap()
            };
            e[64..72].copy_from_slice(&ptr.to_ne_bytes());
            names.extend(name);
            e[72..76].copy_from_slice(&p.base_priority.to_ne_bytes());
            e[80..88].copy_from_slice(&u64::from(p.pid).to_ne_bytes());
            e[88..96].copy_from_slice(&u64::from(p.ppid).to_ne_bytes());
            e[96..100].copy_from_slice(&p.handles.to_ne_bytes());
            e[100..104].copy_from_slice(&p.session.to_ne_bytes());
            e[120..128].copy_from_slice(&p.virtual_size.to_ne_bytes());
            e[128..132].copy_from_slice(&p.page_faults.to_ne_bytes());
            e[144..152].copy_from_slice(&p.working_set.to_ne_bytes());
            e[200..208].copy_from_slice(&p.private.to_ne_bytes());
            e[208..216].copy_from_slice(&p.read_ops.to_ne_bytes());
            e[216..224].copy_from_slice(&p.write_ops.to_ne_bytes());
            e[232..240].copy_from_slice(&p.read_bytes.to_ne_bytes());
            e[240..248].copy_from_slice(&p.write_bytes.to_ne_bytes());
            for (t, &(switches, state, reason)) in threads.iter().enumerate() {
                let th = &mut e[PROCESS_SIZE + t * THREAD_SIZE..];
                th[64..68].copy_from_slice(&switches.to_ne_bytes());
                th[68..72].copy_from_slice(&state.to_ne_bytes());
                th[72..76].copy_from_slice(&reason.to_ne_bytes());
            }
            at += sizes[i];
        }
        buf.extend(names);
        buf
    }
}

fn raw(pid: u32, ppid: u32, name: &str) -> Raw {
    Raw {
        pid,
        ppid,
        name: name.into(),
        create: 133_000_000_000_000_000 + i64::from(pid),
        user: 10_000 * i64::from(pid),
        kernel: 5_000,
        cycles: 1_000_000 * u64::from(pid),
        handles: 100 + pid,
        session: 1,
        base_priority: 8,
        working_set: 1 << 20,
        private: 2 << 20,
        virtual_size: 1 << 40,
        page_faults: 77,
        read_ops: 1,
        write_ops: 2,
        read_bytes: 3,
        write_bytes: 4,
        ..Raw::default()
    }
}

#[test]
fn processes_and_their_threads() {
    let base = 0x1_0000_0000;
    let mut idle = raw(0, 0, "");
    idle.create = 0;
    let running = raw(1234, 0, "explorer.exe");
    let suspended = raw(5678, 1234, "notepad.exe");
    let b = Builder {
        entries: vec![
            (idle.clone(), vec![(10, 2, 0)]),
            (running.clone(), vec![(3, WAITING, SUSPENDED), (4, 2, 0)]),
            (
                suspended.clone(),
                vec![(1, WAITING, SUSPENDED), (2, WAITING, SUSPENDED)],
            ),
        ],
    };
    let got = parse_processes(&b.build(base), base);
    assert_eq!(got.len(), 3);
    let expect = |mut r: Raw, threads: u32, switches: u64, susp: bool| {
        r.threads = threads;
        r.context_switches = switches;
        r.suspended = susp;
        r
    };
    assert_eq!(got[0], expect(idle, 1, 10, false));
    assert_eq!(got[1], expect(running, 2, 7, false));
    assert_eq!(got[2], expect(suspended, 2, 3, true));
}

#[test]
fn damaged_lists_end() {
    let base = 0x5000;
    let b = Builder {
        entries: vec![
            (raw(4, 0, "System"), vec![]),
            (raw(8, 4, "smss.exe"), vec![(1, 2, 0)]),
        ],
    };
    let mut buf = b.build(base);
    // Cut short in the second entry: the first.
    let cut = &buf[..PROCESS_SIZE + 10];
    let got = parse_processes(cut, base);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].pid, 4);
    // A name pointing outside the buffer: no name.
    buf[64..72].copy_from_slice(&1u64.to_ne_bytes());
    assert_eq!(parse_processes(&buf, base)[0].name, "");
    // An entry pointing back to itself: once.
    buf[0..4].copy_from_slice(&0u32.to_ne_bytes());
    let mut looped = buf.clone();
    looped[PROCESS_SIZE..PROCESS_SIZE + 4].copy_from_slice(&0u32.to_ne_bytes());
    assert_eq!(parse_processes(&looped, base).len(), 1);
    // A thread count past the buffer: not suspended, and no panic.
    let mut many = b.build(base);
    many[4..8].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert!(!parse_processes(&many, base)[0].suspended);
    assert!(parse_processes(&[], base).is_empty());
}

#[test]
fn times() {
    assert_eq!(filetime_ns(0), 0);
    assert_eq!(filetime_ns(EPOCH_DIFFERENCE), 0);
    assert_eq!(filetime_ns(EPOCH_DIFFERENCE + 1), 100);
    // 2023-11-14T22:13:20Z.
    assert_eq!(
        filetime_ns(EPOCH_DIFFERENCE + 17_000_000_000_000_000),
        1_700_000_000_000_000_000
    );
    assert_eq!(hundred_ns(15), 1500);
    assert_eq!(hundred_ns(-1), 0);
}

#[test]
fn sids() {
    let system = [1u8, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
    assert_eq!(sid_len(&system), Some(12));
    assert_eq!(sid_string(&system).as_deref(), Some("S-1-5-18"));
    let user: Vec<u8> = [1u8, 5, 0, 0, 0, 0, 0, 5]
        .into_iter()
        .chain(
            [21u32, 1, 2, 3, 1001]
                .into_iter()
                .flat_map(u32::to_ne_bytes),
        )
        .collect();
    assert_eq!(sid_string(&user).as_deref(), Some("S-1-5-21-1-2-3-1001"));
    let label = |rid: u32| -> Vec<u8> {
        [1u8, 1, 0, 0, 0, 0, 0, 16]
            .into_iter()
            .chain(rid.to_ne_bytes())
            .collect()
    };
    assert_eq!(integrity_of(&label(0x2000)), Some("Medium"));
    assert_eq!(integrity_of(&label(0x2100)), Some("Medium Plus"));
    assert_eq!(integrity_of(&label(0x3000)), Some("High"));
    assert_eq!(integrity_of(&label(0x4000)), Some("System"));
    assert_eq!(integrity_of(&label(0x1000)), Some("Low"));
    assert_eq!(integrity_of(&label(0)), Some("Untrusted"));
    assert_eq!(integrity_of(&label(0x5000)), Some("Protected"));
    // Not SIDs.
    assert_eq!(sid_len(&[2, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0]), None);
    assert_eq!(sid_len(&[1, 2, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0]), None);
    assert_eq!(integrity_of(&[1, 0, 0, 0, 0, 0, 0, 16]), None);
    assert_eq!(sid_string(&[]), None);
}

#[test]
fn structures_pointing_into_their_buffers() {
    let base = 0x7000u64;
    // TOKEN_USER: { Sid, Attributes }, the SID after it.
    let mut buf = vec![0u8; 16];
    buf[0..8].copy_from_slice(&(base + 16).to_ne_bytes());
    buf.extend([1u8, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0, 0xee]);
    assert_eq!(sid_in(&buf, base), Some(&buf[16..28]));
    assert_eq!(sid_in(&buf, base + 100), None);
    // UNICODE_STRING: { Length, MaximumLength, Buffer }, its characters
    // after it.
    let text: Vec<u8> = "C:\\x.exe -y"
        .encode_utf16()
        .flat_map(u16::to_ne_bytes)
        .collect();
    let mut us = vec![0u8; 16];
    us[0..2].copy_from_slice(&u16::try_from(text.len()).unwrap().to_ne_bytes());
    us[8..16].copy_from_slice(&(base + 16).to_ne_bytes());
    us.extend(&text);
    assert_eq!(
        unicode_string_in(&us, base).as_deref(),
        Some("C:\\x.exe -y")
    );
    assert_eq!(unicode_string_in(&us[..20], base), None);
    assert_eq!(unicode_string_in(&[0u8; 16], base).as_deref(), Some(""));
}

#[test]
fn machines() {
    assert_eq!(machine_name(0x8664), Some("x64"));
    assert_eq!(machine_name(0xaa64), Some("arm64"));
    assert_eq!(machine_name(0x14c), Some("x86"));
    assert_eq!(machine_name(0x1c4), Some("arm"));
    assert_eq!(machine_name(0), None);
}
