use super::*;

#[test]
fn sizes() {
    assert_eq!(bytes(0), "0 B");
    assert_eq!(bytes(1023), "1023 B");
    assert_eq!(bytes(1024), "1.0 KiB");
    assert_eq!(bytes(1536 * 1024), "1.5 MiB");
    assert_eq!(bytes(u64::MAX), "16.0 EiB");
    assert_eq!(truncate("abcdef", 4), "abc…");
    assert_eq!(truncate("abc", 4), "abc");
}

#[test]
fn options() {
    let o = |v: &[&str]| list_options(&v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    let x = o(&[
        "-sort",
        "cpu",
        "-n",
        "5",
        "-json",
        "-interval",
        "0",
        "-filter",
        "Code",
    ])
    .unwrap();
    assert_eq!(x.sort, Some(Sort::Cpu));
    assert_eq!(x.limit, Some(5));
    assert!(x.json && !x.tree);
    assert!(x.interval.is_zero());
    assert_eq!(x.filter.as_deref(), Some("code"));
    assert!(o(&["-tree", "-sort", "pid"]).is_err());
    assert!(o(&["-sort", "nope"]).is_err());
    assert!(o(&["-n"]).is_err());
    assert!(o(&["-bogus"]).is_err());
}

#[test]
fn naming_processes() {
    let none = || -> std::io::Result<Snapshot> { Err(std::io::Error::other("not taken")) };
    let s = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    // With starts given: no snapshot taken.
    assert_eq!(
        keys(&s(&["5@10", "6@0"]), none),
        Ok(vec![Key { pid: 5, start: 10 }, Key { pid: 6, start: 0 }])
    );
    assert!(keys(&s(&["x"]), none).is_err());
    assert!(keys(&s(&["5@x"]), none).is_err());
    assert!(keys(&[], none).is_err());
    // By ID: this process, from a snapshot.
    let me = std::process::id().to_string();
    let k = keys(&s(&[&me]), || {
        turbonop_process::Collector::new()?.snapshot()
    })
    .unwrap();
    assert_eq!(k[0].pid, std::process::id());
    assert!(k[0].start > 0);
}
