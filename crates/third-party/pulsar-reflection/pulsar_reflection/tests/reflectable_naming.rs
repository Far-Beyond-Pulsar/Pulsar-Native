#![deny(non_upper_case_globals)]

use pulsar_reflection::Reflectable;

// These names intentionally exercise the existing case-preserving generated
// identifiers.  The derive must suppress the global-name lint locally rather
// than uppercasing names and making `Foo`/`FOO` collide.
#[derive(Clone, Reflectable)]
struct Foo {
    value: i32,
}

#[derive(Clone, Reflectable)]
#[allow(non_camel_case_types, dead_code)]
struct FOO {
    value: i32,
}

#[derive(Clone, Reflectable)]
#[allow(dead_code)]
enum HealthState {
    Good,
    Bad,
}

#[test]
fn derived_type_info_is_available_with_global_lints_denied() {
    assert_eq!(<Foo as Reflectable>::type_info().type_name, "Foo");
    assert_eq!(<FOO as Reflectable>::type_info().type_name, "FOO");
    assert_eq!(
        <HealthState as Reflectable>::type_info().type_name,
        "HealthState"
    );
}
