//! Every case here was run with `lib.exe` 14.52 or `llvm-lib` (LLVM
//! c74b358f44c3); the expected orders are what `llvm-ar t` printed for their
//! output.

use super::*;

const B: &str = r"X:\dir\b.obj";

fn obj(p: &str) -> Input {
    Input::Object(p.to_string())
}

fn lib(p: &str, members: &[&str]) -> Input {
    Input::Library(
        p.to_string(),
        members.iter().map(ToString::to_string).collect(),
    )
}

/// names.lib, from `lib a.obj sub\a.obj X:\dir\b.obj ./c.obj`.
fn names() -> Input {
    lib("names.lib", &[r".\c.obj", B, r"sub\a.obj", "a.obj"])
}

fn l1() -> Input {
    lib("l1.lib", &["b.obj", "a.obj"])
}

fn l2() -> Input {
    lib("l2.lib", &[r"sub\a.obj", "c.obj"])
}

fn l3() -> Input {
    lib("l3.lib", &["a.obj"])
}

fn order(inputs: &[Input], remove: &[&str]) -> Vec<String> {
    let remove: Vec<String> = remove.iter().map(ToString::to_string).collect();
    let o = lib_order(inputs, &remove, false).unwrap();
    o.members.iter().map(|&s| stored_name(inputs, s)).collect()
}

fn warnings(inputs: &[Input], remove: &[&str]) -> Vec<String> {
    let remove: Vec<String> = remove.iter().map(ToString::to_string).collect();
    let o = lib_order(inputs, &remove, false).unwrap();
    o.warnings.iter().map(ToString::to_string).collect()
}

#[test]
fn objects_are_reversed_and_stored_with_backslashes() {
    let inputs = [obj("a.obj"), obj("sub/a.obj"), obj(B), obj("./c.obj")];
    assert_eq!(order(&inputs, &[]), [r".\c.obj", B, r"sub\a.obj", "a.obj"]);
}

#[test]
fn libraries_first_then_objects_reversed() {
    // lib /OUT:merge.lib a.obj names.lib c.obj
    let inputs = [obj("a.obj"), names(), obj("c.obj")];
    assert_eq!(
        order(&inputs, &[]),
        [r"sub\a.obj", B, r".\c.obj", "c.obj", "a.obj"]
    );
    let o = lib_order(&inputs, &[], false).unwrap();
    assert_eq!(o.messages, ["Replacing a.obj"]);
    // lib /OUT:two.lib l1.lib a2.obj l2.lib
    assert_eq!(
        order(&[l1(), obj("a2.obj"), l2()], &[]),
        ["a.obj", "b.obj", "c.obj", r"sub\a.obj", "a2.obj"]
    );
    // lib /OUT:w.lib names.lib a2.obj
    assert_eq!(
        order(&[names(), obj("a2.obj")], &[]),
        ["a.obj", r"sub\a.obj", B, r".\c.obj", "a2.obj"]
    );
}

#[test]
fn updating_a_library_in_place() {
    // lib b.lib a.obj, where b.lib came from lib b.obj c.obj.
    let first = order(&[lib("b.lib", &["c.obj", "b.obj"]), obj("a.obj")], &[]);
    assert_eq!(first, ["b.obj", "c.obj", "a.obj"]);
    // Then lib b.lib b.obj.
    let members: Vec<&str> = first.iter().map(String::as_str).collect();
    let inputs = [lib("b.lib", &members), obj("b.obj")];
    assert_eq!(order(&inputs, &[]), ["a.obj", "c.obj", "b.obj"]);
    assert_eq!(
        lib_order(&inputs, &[], false).unwrap().messages,
        ["Replacing b.obj"]
    );
}

#[test]
fn replacement_across_libraries() {
    // Two libraries' members with one name are both kept...
    assert_eq!(order(&[l1(), l3()], &[]), ["a.obj", "b.obj", "a.obj"]);
    // ...and an object replaces both, saying so once.
    let inputs = [l1(), l3(), obj("a.obj")];
    assert_eq!(order(&inputs, &[]), ["b.obj", "a.obj"]);
    assert_eq!(
        lib_order(&inputs, &[], false).unwrap().messages,
        ["Replacing a.obj"]
    );
    assert_eq!(order(&[l1(), obj("a.obj")], &[]), ["b.obj", "a.obj"]);
}

#[test]
fn repeated_inputs() {
    let inputs = [obj("a.obj"), obj("b.obj"), obj("a.obj")];
    assert_eq!(order(&inputs, &[]), ["b.obj", "a.obj"]);
    assert_eq!(
        warnings(&inputs, &[]),
        ["a.obj : warning LNK4042: object specified more than once; extras ignored"]
    );
    let inputs = [obj("a.obj"), obj("A.OBJ")];
    assert_eq!(order(&inputs, &[]), ["a.obj"]);
    assert_eq!(
        warnings(&inputs, &[]),
        ["A.OBJ : warning LNK4042: object specified more than once; extras ignored"]
    );
    // A library twice counts once, without a warning.
    assert_eq!(order(&[l1(), l1()], &[]), ["a.obj", "b.obj"]);
    assert!(warnings(&[l1(), l1()], &[]).is_empty());
}

#[test]
fn verbose_messages() {
    // lib /VERBOSE l1.lib c.obj a.obj
    let inputs = [l1(), obj("c.obj"), obj("a.obj")];
    let o = lib_order(&inputs, &[], true).unwrap();
    assert_eq!(
        o.messages,
        [
            "Replacing a.obj",
            "Appending b.obj",
            "Appending a.obj",
            "Appending c.obj"
        ]
    );
}

#[test]
fn remove() {
    // From every library, without case.
    assert_eq!(order(&[l1(), l3()], &["a.obj"]), ["b.obj"]);
    assert_eq!(order(&[names()], &[r"SUB\A.OBJ"]), ["a.obj", B, r".\c.obj"]);
    // Never an object on the command line.
    let inputs = [l1(), obj("c.obj")];
    assert_eq!(order(&inputs, &["c.obj"]), ["a.obj", "b.obj", "c.obj"]);
    assert_eq!(
        warnings(&inputs, &["c.obj"]),
        ["LINK : warning LNK4014: cannot find member object c.obj"]
    );
    // After replacement: names.lib /REMOVE:a.obj a.obj keeps the object.
    let inputs = [names(), obj("a.obj")];
    assert_eq!(
        order(&inputs, &["a.obj"]),
        [r"sub\a.obj", B, r".\c.obj", "a.obj"]
    );
    assert_eq!(
        warnings(&inputs, &["a.obj"]),
        ["LINK : warning LNK4014: cannot find member object a.obj"]
    );
    // Removing everything is fatal.
    let e = lib_order(&[l3()], &["a.obj".to_string()], false).unwrap_err();
    assert_eq!(
        e.to_string(),
        "LINK : fatal error LNK1160: library with zero objects not allowed"
    );
}

#[test]
fn llvm_lib() {
    let names_of = |inputs: &[Input]| -> Vec<String> {
        llvm_order(inputs)
            .members
            .iter()
            .map(|&s| match s {
                Source::Object(i) => match &inputs[i] {
                    Input::Object(p) => p.clone(),
                    _ => unreachable!(),
                },
                Source::Member(..) => stored_name(inputs, s),
            })
            .collect()
    };
    // llvm-lib /OUT:two-llvm.lib l1.lib a2.obj l2.lib
    assert_eq!(
        names_of(&[l1(), obj("a2.obj"), l2()]),
        ["c.obj", r"sub\a.obj", "a2.obj", "a.obj", "b.obj"]
    );
    // llvm-lib /OUT:merge-llvm.lib a.obj names.lib c.obj: no replacement.
    assert_eq!(
        names_of(&[obj("a.obj"), names(), obj("c.obj")]),
        ["c.obj", "a.obj", r"sub\a.obj", B, r".\c.obj", "a.obj"]
    );
    // The same path twice counts once; another spelling doesn't.
    assert_eq!(
        names_of(&[obj("a.obj"), obj("b.obj"), obj("a.obj")]),
        ["b.obj", "a.obj"]
    );
    assert_eq!(
        names_of(&[obj("a.obj"), obj("./a.obj")]),
        ["./a.obj", "a.obj"]
    );
}
