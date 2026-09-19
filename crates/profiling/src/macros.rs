//! Profiling macros

#[macro_export]
macro_rules! profile_scope {
    ($name:literal) => {
        let _profile_guard = $crate::scope::ProfileScope::new_static($name);
    };
    ($name:expr) => {
        let _profile_guard = $crate::scope::ProfileScope::new($name);
    };
}

#[macro_export]
macro_rules! profile_scope_with_context {
    ($name:literal, $context:expr) => {
        let _profile_guard = $crate::scope::ProfileScope::new_with_context($name, ($context).clone());
    };
    ($name:expr, $context:expr) => {
        let _profile_guard = $crate::scope::ProfileScope::new_with_context($name, ($context).clone());
    };
}

#[macro_export]
macro_rules! profile_scope_loc {
    ($name:literal) => {
        let _profile_guard = $crate::scope::ProfileScope::new_static_with_location(
            $name,
            Some(format!("{}:{}", file!(), line!())),
        );
    };
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
        let _profile_guard = $crate::scope::ProfileScope::new_static(module_path!());
    };
}
