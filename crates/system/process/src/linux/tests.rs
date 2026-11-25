use super::*;

#[test]
fn stat() {
    let text = "1234 (bash) S 1000 1234 1234 34816 1300 4194304 2500 10 3 0 150 40 0 0 20 0 1 0 98765 12345678 900 18446744073709551615 1 1 0 0 0 0 65536 3670020 1266777851 0 0 0 17 3 0 0 0 0 0\n";
    let s = parse_stat(text).unwrap();
    assert_eq!(
        s,
        Stat {
            pid: 1234,
            comm: "bash".into(),
            state: b'S',
            ppid: 1000,
            session: 1234,
            flags: 4_194_304,
            minflt: 2500,
            majflt: 3,
            utime: 150,
            stime: 40,
            nice: 0,
            threads: 1,
            starttime: 98765,
            vsize: 12_345_678,
        }
    );
}

#[test]
fn stat_names_with_parentheses_and_spaces() {
    // (A name is the bytes between the first `(` and the last `)`.)
    let text = "77 (a) b (c)) R 1 77 77 0 -1 2097216 0 0 0 0 0 0 0 0 20 -5 4 0 5 0 0";
    let s = parse_stat(text).unwrap();
    assert_eq!(s.comm, "a) b (c)");
    assert_eq!(s.state, b'R');
    assert_eq!(s.nice, -5);
    assert_eq!(s.threads, 4);
    assert_eq!(s.flags & PF_KTHREAD, PF_KTHREAD);
    // (Cut short, or not a number: nothing.)
    assert_eq!(parse_stat("77 (a) R 1 2"), None);
    assert_eq!(
        parse_stat("x (a) R 1 77 77 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 5 0 0"),
        None
    );
    assert_eq!(parse_stat(""), None);
    assert_eq!(parse_stat("1 )("), None);
}

#[test]
fn states() {
    assert_eq!(state_of(b'R'), State::Running);
    assert_eq!(state_of(b'S'), State::Sleeping);
    assert_eq!(state_of(b'I'), State::Sleeping);
    assert_eq!(state_of(b'D'), State::DiskSleep);
    assert_eq!(state_of(b'T'), State::Stopped);
    assert_eq!(state_of(b't'), State::Stopped);
    assert_eq!(state_of(b'Z'), State::Zombie);
    assert_eq!(state_of(b'X'), State::Dead);
}

#[test]
fn statm() {
    assert_eq!(
        parse_statm("5000 1200 300 10 0 900 0\n"),
        Some((5000, 1200, 300))
    );
    assert_eq!(parse_statm("5000 1200"), None);
}

#[test]
fn io() {
    let text = "rchar: 100\nwchar: 200\nsyscr: 3\nsyscw: 4\nread_bytes: 4096\nwrite_bytes: 8192\ncancelled_write_bytes: 0\n";
    assert_eq!(
        parse_io(text),
        Io {
            rchar: 100,
            wchar: 200,
            syscr: 3,
            syscw: 4,
            read_bytes: 4096,
            write_bytes: 8192,
            cancelled_write_bytes: 0
        }
    );
    assert_eq!(parse_io("junk\nrchar: x\n"), Io::default());
}

#[test]
fn cmdline() {
    assert_eq!(
        parse_cmdline(b"/bin/sh\0-c\0echo hi\0"),
        ["/bin/sh", "-c", "echo hi"]
    );
    assert_eq!(parse_cmdline(b""), Vec::<String>::new());
    // (Empty arguments are kept, but not a rewritten title's padding.)
    assert_eq!(parse_cmdline(b"a\0\0b\0"), ["a", "", "b"]);
    assert_eq!(
        parse_cmdline(b"nginx: worker process\0\0\0\0"),
        ["nginx: worker process"]
    );
    assert_eq!(parse_cmdline(b"no terminator"), ["no terminator"]);
}

#[test]
fn users() {
    let text = "# comment\nroot:x:0:0:root:/root:/bin/bash\nbin:x:1:1::/:/usr/sbin/nologin\ntoor:x:0:0::/:/bin/sh\nbad line\nnan:x:q:0::/:/bin/sh\n";
    let users = parse_passwd(text);
    assert_eq!(users.len(), 2);
    assert_eq!(users[&0], "root");
    assert_eq!(users[&1], "bin");
    assert_eq!(
        parse_status_uids("Name:\tbash\nUid:\t1000\t0\t0\t0\nGid:\t1000\n"),
        Some((1000, 0))
    );
    assert_eq!(parse_status_uids("Name:\tbash\n"), None);
}

#[test]
fn proc_stat() {
    let text = "cpu  100 5 50 1000 20 1 2 3 7 0\ncpu0 50 2 25 500 10 0 1 1 3 0\ncpu1 50 3 25 500 10 1 1 2 4 0\nintr 1 2 3\nctxt 999\nbtime 1700000000\nprocesses 50\n";
    let s = parse_proc_stat(text);
    assert_eq!(
        s,
        ProcStat {
            cpus: 2,
            total: 100 + 5 + 50 + 1000 + 20 + 1 + 2 + 3,
            idle: 1020,
            btime: 1_700_000_000
        }
    );
}

#[test]
fn meminfo() {
    let text = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    8000000 kB\nBuffers:          100 kB\nCached:         2000 kB\nSwapTotal:       4000000 kB\nSwapFree:        3000000 kB\nCommitLimit:    12000000 kB\nCommitted_AS:    9000000 kB\nHugePages_Total:       0\n";
    let m = parse_meminfo(text);
    assert_eq!(m.total, 16_000_000 * 1024);
    assert_eq!(m.available, 8_000_000 * 1024);
    assert_eq!(m.swap_total, 4_000_000 * 1024);
    assert_eq!(m.swap_free, 3_000_000 * 1024);
    assert_eq!(m.commit_limit, Some(12_000_000 * 1024));
    assert_eq!(m.committed, Some(9_000_000 * 1024));
    // (No MemAvailable: free, buffers, and cache.)
    let old = parse_meminfo("MemTotal: 100 kB\nMemFree: 10 kB\nBuffers: 1 kB\nCached: 2 kB\n");
    assert_eq!(old.available, 13 * 1024);
}

#[test]
fn uptime() {
    assert_eq!(
        parse_uptime("12345.67 99999.00\n"),
        Some(12_345_670_000_000)
    );
    assert_eq!(parse_uptime("5\n"), Some(5_000_000_000));
    assert_eq!(parse_uptime("1.000000001"), Some(1_000_000_001));
    assert_eq!(parse_uptime("x"), None);
    assert_eq!(parse_uptime("1.x"), None);
    assert_eq!(parse_uptime(""), None);
}

#[test]
fn auxv() {
    let words = |pairs: &[(u64, u64)], word: usize| -> Vec<u8> {
        pairs
            .iter()
            .flat_map(|&(k, v)| {
                if word == 4 {
                    let mut b = u32::try_from(k).unwrap().to_ne_bytes().to_vec();
                    b.extend(u32::try_from(v).unwrap().to_ne_bytes());
                    b
                } else {
                    let mut b = k.to_ne_bytes().to_vec();
                    b.extend(v.to_ne_bytes());
                    b
                }
            })
            .collect()
    };
    let pairs = [(33, 0x7fff_0000), (6, 4096), (17, 100), (0, 0), (17, 250)];
    assert_eq!(parse_auxv(&words(&pairs, 8), 8), (Some(100), Some(4096)));
    assert_eq!(parse_auxv(&words(&pairs, 4), 4), (Some(100), Some(4096)));
    assert_eq!(parse_auxv(&words(&[(6, 16384)], 8), 8), (None, Some(16384)));
    assert_eq!(parse_auxv(&[1, 2, 3], 8), (None, None));
}

#[test]
fn names() {
    assert_eq!(exe_path("/usr/bin/foo (deleted)"), "/usr/bin/foo");
    assert_eq!(exe_path("/usr/bin/foo"), "/usr/bin/foo");
    // Cut short: the executable's whole name.
    assert_eq!(
        display_name(
            "gnome-shell-cal",
            Some("/usr/libexec/gnome-shell-calendar-server"),
            None
        ),
        "gnome-shell-calendar-server"
    );
    // Or the first argument's.
    assert_eq!(
        display_name(
            "systemd-journal",
            None,
            Some("/lib/systemd/systemd-journald")
        ),
        "systemd-journald"
    );
    // Not cut short, or another name: its own.
    assert_eq!(display_name("bash", Some("/usr/bin/bash5"), None), "bash");
    assert_eq!(
        display_name("python-worker-x", Some("/usr/bin/python3"), None),
        "python-worker-x"
    );
}
