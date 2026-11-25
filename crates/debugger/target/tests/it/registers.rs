use turbonop_target::Registers;

#[test]
fn set_get_forget() {
    let mut r = Registers::new();
    assert!(r.is_empty());
    r.set(30, Some(3));
    r.set(2, Some(1));
    r.set(17, Some(u128::MAX));
    r.set(2, Some(5));
    assert_eq!(r.len(), 3);
    assert_eq!(r.get(2), Some(5));
    assert_eq!(r.get(17), Some(u128::MAX));
    assert_eq!(r.get64(17), Some(u64::MAX));
    assert_eq!(r.get(3), None);
    assert_eq!(r.iter().map(|(n, _)| n).collect::<Vec<_>>(), [2, 17, 30]);
    r.set(17, None);
    r.set(99, None);
    assert_eq!(r.iter().collect::<Vec<_>>(), [(2, 5), (30, 3)]);
}

#[test]
fn unwinders_registers() {
    let mut r = Registers::new();
    r.set(7, Some(0x1000));
    r.set(16, Some(0x4000_1234));
    r.set(17, Some(1 << 100 | 9));
    r.set(600, Some(1)); // Past the unwinders' numbers.
    let u = r.to_unwind();
    assert_eq!(u.get(7), Some(0x1000));
    assert_eq!(u.get(16), Some(0x4000_1234));
    assert_eq!(u.get(17), Some(9));
    assert_eq!(u.get(0), None);
    let back = Registers::from_unwind(&u);
    assert_eq!(
        back.iter().collect::<Vec<_>>(),
        [(7, 0x1000), (16, 0x4000_1234), (17, 9)]
    );
}
