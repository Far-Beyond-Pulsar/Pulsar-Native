use super::*;

struct MockSubsystem {
    id: SubsystemId,
    deps: Vec<SubsystemId>,
    init_called: bool,
}

impl MockSubsystem {
    fn new(id: SubsystemId, deps: Vec<SubsystemId>) -> Self {
        Self { id, deps, init_called: false }
    }
}

impl Subsystem for MockSubsystem {
    fn id(&self) -> SubsystemId { self.id }
    fn dependencies(&self) -> Vec<SubsystemId> { self.deps.clone() }

    fn init(&mut self, _context: &SubsystemContext) -> Result<(), SubsystemError> {
        self.init_called = true;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), SubsystemError> {
        Ok(())
    }
}

#[test]
fn test_simple_dependency_resolution() {
    let mut registry = SubsystemRegistry::new();

    let a = SubsystemId::new("a");
    let b = SubsystemId::new("b");
    let c = SubsystemId::new("c");

    registry.register(MockSubsystem::new(a, vec![])).unwrap();
    registry.register(MockSubsystem::new(b, vec![a])).unwrap();
    registry.register(MockSubsystem::new(c, vec![b])).unwrap();

    let order = registry.resolve_dependencies().unwrap();

    let a_pos = order.iter().position(|&id| id == a).unwrap();
    let b_pos = order.iter().position(|&id| id == b).unwrap();
    let c_pos = order.iter().position(|&id| id == c).unwrap();

    assert!(a_pos < b_pos);
    assert!(b_pos < c_pos);
}

#[test]
fn test_cycle_detection() {
    let mut registry = SubsystemRegistry::new();

    let a = SubsystemId::new("a");
    let b = SubsystemId::new("b");

    registry.register(MockSubsystem::new(a, vec![b])).unwrap();
    registry.register(MockSubsystem::new(b, vec![a])).unwrap();

    let result = registry.resolve_dependencies();
    assert!(matches!(result, Err(SubsystemError::DependencyCycle { .. })));
}

#[test]
fn test_missing_dependency() {
    let mut registry = SubsystemRegistry::new();

    let a = SubsystemId::new("a");
    let b = SubsystemId::new("b");

    registry.register(MockSubsystem::new(b, vec![a])).unwrap();

    let result = registry.resolve_dependencies();
    assert!(matches!(result, Err(SubsystemError::MissingDependency { .. })));
}

#[test]
fn test_register_boxed() {
    let mut registry = SubsystemRegistry::new();
    let id = SubsystemId::new("boxed");

    registry
        .register_boxed(Box::new(MockSubsystem::new(id, vec![])))
        .unwrap();

    assert!(registry.contains(id));
}

#[test]
fn test_merge() {
    let mut reg1 = SubsystemRegistry::new();
    let mut reg2 = SubsystemRegistry::new();

    let a = SubsystemId::new("a");
    let b = SubsystemId::new("b");

    reg1.register(MockSubsystem::new(a, vec![])).unwrap();
    reg2.register(MockSubsystem::new(b, vec![])).unwrap();

    reg1.merge(reg2);
    assert!(reg1.contains(a));
    assert!(reg1.contains(b));
}

#[test]
fn test_downcast_after_register() {
    let mut registry = SubsystemRegistry::new();
    let id = SubsystemId::new("downcast_test");
    registry.register(MockSubsystem::new(id, vec![])).unwrap();

    let ss = registry.get(id).unwrap();
    let any: &dyn std::any::Any = ss;
    let downcasted = any.downcast_ref::<MockSubsystem>();
    assert!(downcasted.is_some());
}
