use turbonop_target::linux_maps::{MappedModule, mappings, modules};

#[test]
fn lines() {
    let text = b"\
00400000-00452000 r-xp 00000000 08:02 173521      /usr/bin/dbus-daemon
00e03000-00e24000 rw-p 00000000 00:00 0           [heap]
7f0000000000-7f0000001000 ---p 00000000 00:00 0
not a line
f0-10 r--p 0 0 0 backwards
";
    let m = mappings(text);
    assert_eq!(m.len(), 3);
    assert_eq!(
        (m[0].start, m[0].end, m[0].offset),
        (0x40_0000, 0x45_2000, 0)
    );
    assert!(m[0].readable && m[0].executable && !m[0].writable);
    assert_eq!(m[0].path, "/usr/bin/dbus-daemon");
    assert_eq!(m[1].path, "[heap]");
    assert!(m[1].writable);
    assert_eq!(m[2].path, "");
    assert!(!m[2].readable);
}

#[test]
fn paths_with_spaces_and_deleted_files() {
    let text = b"\
7f0000000000-7f0000002000 r-xp 00000000 08:01 5 /opt/my app/lib x.so (deleted)
";
    assert_eq!(
        modules(text),
        [MappedModule {
            path: "/opt/my app/lib x.so (deleted)".into(),
            base: 0x7F00_0000_0000,
            size: 0x2000
        }]
    );
}

#[test]
fn modules_span_their_files_mappings() {
    let text = b"\
1000-2000 r--p 00000000 08:01 1 /a.so
2000-3000 r-xp 00001000 08:01 1 /a.so
3000-4000 rw-p 00000000 00:00 0
4000-5000 rw-p 00003000 08:01 1 /a.so
5000-6000 r--p 00000000 08:01 2 /data.bin
6000-7000 r-xp 00000000 08:01 3 /b.so
";
    assert_eq!(
        modules(text),
        [
            MappedModule {
                path: "/a.so".into(),
                base: 0x1000,
                size: 0x4000
            },
            MappedModule {
                path: "/b.so".into(),
                base: 0x6000,
                size: 0x1000
            },
        ]
    );
}

#[test]
fn not_utf8() {
    let mut text = b"1000-2000 r-xp 00000000 08:01 1 /a.so\n".to_vec();
    text.extend(b"2000-3000 r-xp 00000000 08:01 1 /b\xff.so\n");
    assert_eq!(modules(&text).len(), 1);
}
