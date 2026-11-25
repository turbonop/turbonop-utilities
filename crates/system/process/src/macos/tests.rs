use super::*;

fn put(b: &mut [u8], at: usize, v: &[u8]) {
    b[at..at + v.len()].copy_from_slice(v);
}

#[test]
fn bsdinfo() {
    let mut b = vec![0u8; BSDINFO_SIZE];
    put(&mut b, 0, &0x10u32.to_ne_bytes());
    put(&mut b, 4, &2u32.to_ne_bytes());
    put(&mut b, 12, &4321u32.to_ne_bytes());
    put(&mut b, 16, &1u32.to_ne_bytes());
    put(&mut b, 20, &501u32.to_ne_bytes());
    put(&mut b, 28, &502u32.to_ne_bytes());
    put(&mut b, 48, b"WindowServer\0");
    put(&mut b, 64, b"WindowServer-long-name\0");
    put(&mut b, 100, &4321u32.to_ne_bytes());
    put(&mut b, 116, &(-10i32).to_ne_bytes());
    put(&mut b, 120, &1_700_000_000u64.to_ne_bytes());
    put(&mut b, 128, &250_000u64.to_ne_bytes());
    let i = parse_bsdinfo(&b).unwrap();
    assert_eq!(
        i,
        BsdInfo {
            status: 2,
            pid: 4321,
            ppid: 1,
            uid: 501,
            ruid: 502,
            comm: "WindowServer".into(),
            name: "WindowServer-long-name".into(),
            pgid: 4321,
            nice: -10,
            start_us: 1_700_000_000_250_000,
            flags: 0x10,
        }
    );
    // A name filling its field, without a NUL.
    put(&mut b, 48, b"0123456789abcdef");
    assert_eq!(parse_bsdinfo(&b).unwrap().comm, "0123456789abcdef");
    assert_eq!(parse_bsdinfo(&b[..BSDINFO_SIZE - 1]), None);
}

#[test]
fn taskinfo() {
    let mut b = vec![0u8; TASKINFO_SIZE];
    put(&mut b, 0, &(1u64 << 36).to_ne_bytes());
    put(&mut b, 8, &(50u64 << 20).to_ne_bytes());
    put(&mut b, 16, &1_000_000u64.to_ne_bytes());
    put(&mut b, 24, &250_000u64.to_ne_bytes());
    put(&mut b, 52, &900i32.to_ne_bytes());
    put(&mut b, 56, &12i32.to_ne_bytes());
    put(&mut b, 80, &3000i32.to_ne_bytes());
    put(&mut b, 84, &9i32.to_ne_bytes());
    put(&mut b, 88, &1i32.to_ne_bytes());
    put(&mut b, 92, &31i32.to_ne_bytes());
    assert_eq!(
        parse_taskinfo(&b),
        Some(TaskInfo {
            virtual_size: 1 << 36,
            resident: 50 << 20,
            user: 1_000_000,
            system: 250_000,
            faults: 900,
            pageins: 12,
            context_switches: 3000,
            threads: 9,
            running: 1,
            priority: 31,
        })
    );
    // (A negative count, as a wrapped 32-bit counter shows: 0.)
    put(&mut b, 80, &(-1i32).to_ne_bytes());
    assert_eq!(parse_taskinfo(&b).unwrap().context_switches, 0);
    assert_eq!(parse_taskinfo(&b[..10]), None);
}

#[test]
fn rusage() {
    let mut b = vec![0u8; RUSAGE_V2_SIZE];
    put(&mut b, 72, &(80u64 << 20).to_ne_bytes());
    put(&mut b, 144, &4096u64.to_ne_bytes());
    put(&mut b, 152, &8192u64.to_ne_bytes());
    assert_eq!(
        parse_rusage(&b),
        Some(Rusage {
            footprint: 80 << 20,
            disk_read: 4096,
            disk_written: 8192
        })
    );
    assert_eq!(parse_rusage(&b[..100]), None);
}

#[test]
fn states() {
    assert_eq!(state_of(1), State::Starting);
    assert_eq!(state_of(2), State::Running);
    assert_eq!(state_of(3), State::Sleeping);
    assert_eq!(state_of(4), State::Stopped);
    assert_eq!(state_of(5), State::Zombie);
}

#[test]
fn procargs() {
    let mut b = 3i32.to_ne_bytes().to_vec();
    b.extend(b"/bin/zsh\0\0\0\0-zsh\0-c\0echo hi\0PATH=/bin\0HOME=/\0");
    assert_eq!(
        parse_procargs(&b),
        Some((
            "/bin/zsh".to_owned(),
            vec!["-zsh".to_owned(), "-c".to_owned(), "echo hi".to_owned()]
        ))
    );
    // (Fewer arguments than argc says: those there are.)
    let mut short = 5i32.to_ne_bytes().to_vec();
    short.extend(b"/x\0a\0");
    assert_eq!(
        parse_procargs(&short),
        Some(("/x".to_owned(), vec!["a".to_owned(), String::new()]))
    );
    assert_eq!(parse_procargs(&(-1i32).to_ne_bytes()), None);
    assert_eq!(parse_procargs(&[1, 0]), None);
    let mut no_nul = 1i32.to_ne_bytes().to_vec();
    no_nul.extend(b"/x");
    assert_eq!(parse_procargs(&no_nul), None);
}

#[test]
fn vm_stats() {
    let mut b = vec![0u8; 152];
    for (at, v) in [
        (0, 100u32),
        (4, 200),
        (8, 300),
        (12, 400),
        (88, 5),
        (92, 6),
        (128, 70),
        (136, 80),
    ] {
        put(&mut b, at, &v.to_ne_bytes());
    }
    let s = parse_vm_stats(&b).unwrap();
    assert_eq!(
        s,
        VmStats {
            free: 100,
            active: 200,
            inactive: 300,
            wired: 400,
            speculative: 6,
            purgeable: 5,
            compressor: 70,
            external: 80
        }
    );
    assert_eq!(s.available(), 100 + 80 + 5);
    assert_eq!(parse_vm_stats(&b[..100]), None);
}
