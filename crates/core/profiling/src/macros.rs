//! Profiling macros

#[macro_export]
macro_rules! profile_scope {
    ($name:expr) => {
        let _profile_guard = $crate::scope::ProfileScope::new($name);
    };
}

#[macro_export]
macro_rules! profile_scope_loc {
    ($name:expr) => {
        let _profile_guard = $crate::scope::ProfileScope::new_with_location(
            $name,
            Some(format!("{}:{}", file!(), line!())),
        );
    };
}

#[macro_export]
macro_rules! profile_function {
    () => {
        $crate::profile_scope!(module_path!());
    };
}
