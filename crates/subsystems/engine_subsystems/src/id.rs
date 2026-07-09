/// Unique identifier for a subsystem.
///
/// Opaque `&'static str` handle — cheap to copy and compare.
/// Consumers define their own constants and use them with the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubsystemId(&'static str);

impl SubsystemId {
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for SubsystemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
