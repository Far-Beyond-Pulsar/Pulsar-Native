# Reflection system

Pulsar has a two-tier type system: compile-time types (known at link time via
`inventory`) and dynamic types (composed at runtime).

## Compile-time types

Types implement `Reflectable` (via `#[derive(Reflectable)]` or manual impl):

```rust
pub trait Reflectable: Any + Send + Sync {
    fn type_info() -> &'static RuntimeTypeInfo where Self: Sized;
    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()>;
    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self>
        where Self: Sized;
    fn clone_any(&self) -> Box<dyn Any>;
}
```

Deriving generates an `inventory::submit!(RuntimeTypeRegistration { ... })`
call that auto-populates the global `RUNTIME_TYPE_REGISTRY` at link time.

## RuntimeTypeInfo

Every reflected type has a static descriptor:

```rust
pub struct RuntimeTypeInfo {
    pub type_id: TypeId,
    pub type_name: &'static str,       // e.g. "f64", "MyStruct"
    pub size: usize,
    pub align: usize,
    pub structure: TypeStructure,       // Primitive | String | Wrapper | Struct | Enum | Wildcard
    pub color: Option<&'static str>,   // Optional display color
}
```

`TypeStructure` variants:
- `Primitive` — plain data (f64, i32, bool, etc.)
- `String` — string types
- `Wrapper { wrapper_kind: WrapperType, inner: &'static RuntimeTypeInfo }`
- `Struct { fields: &'static [FieldInfo] }`
- `Enum { variants: &'static [&'static str] }`
- `Wildcard` — type-erased placeholder

`FieldInfo`: `{ name: &'static str, type_info: &'static RuntimeTypeInfo, offset: usize }`

`WrapperType`: `Vec | Box | Arc | Rc | Option | Result | HashMap | HashSet | Custom(&'static str)`

## Registry access

```rust
// By Rust type:
let info = RuntimeTypeInfo::of::<MyType>();

// By name (thread-safe lookup):
let info = RUNTIME_TYPE_REGISTRY.get_by_name("MyType");

// By TypeId:
let info = RUNTIME_TYPE_REGISTRY.get_by_id(&TypeId::of::<MyType>());

// Serialize any reflectable type to JSON:
let json = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(&value)?;

// Deserialize:
let value = RUNTIME_TYPE_REGISTRY.deserialize_json_for_type::<MyType>(json)?;
```

## EngineClass

Engine components implement the `EngineClass` trait (derived via
`#[derive(EngineClass)]`):

```rust
pub trait EngineClass: Any + Send + Sync {
    fn class_name() -> &'static str where Self: Sized;
    fn get_properties(&self) -> Vec<PropertyMetadata>;      // Reflected properties
    fn get_methods() -> Vec<MethodMetadata> where Self: Sized; // Blueprint-callable methods
    fn create_default() -> Box<dyn EngineClass> where Self: Sized;
    fn clone_boxed(&self) -> Box<dyn EngineClass>;
}
```

`PropertyMetadata` carries:
- The property name and `RuntimeTypeInfo`
- Getter/setter closures for reading/writing the field on any `Box<dyn EngineClass>`
- Default value and category for the editor UI

`MethodMetadata` carries:
- The method name, parameter types, return type
- A caller closure that invokes the method on any `Box<dyn EngineClass>`
- Input/output descriptions for the blueprint editor

The `EngineClassRegistry` (global, inventory-populated) provides lookup by name
and class name-to-factory mapping.

## Dynamic types

Types composed at runtime via `DynamicTypeBuilder`:

```rust
let mut builder = DynamicTypeBuilder::new("MyDynamicType");
builder.add_field("position", RuntimeTypeInfo::of::<Vec3>());
builder.add_field("health", RuntimeTypeInfo::of::<f32>());
let type_info = builder.build();

// Create instances:
let mut value = DynamicValue::new(&type_info);
value.set_field("position", Vec3::new(1.0, 2.0, 3.0));
let pos: Vec3 = value.get_field_typed("position").unwrap();
```

Dynamic types are registered in `DYNAMIC_TYPE_REGISTRY` (global `LazyLock`,
auto-assigns UUIDs). User-defined `.alias.json` files are scanned by
`engine_fs::UserTypeRegistry` and registered here.

## ComponentRuntimeBehavior

Runtime components implement this trait to participate in the ECS tick:

```rust
pub trait ComponentRuntimeBehavior {
    const CLASS_NAME: &'static str;
    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    );
}
```

Registered via `inventory::submit!(RuntimeBehaviorRegistration { ... })`.
The central system iterates all component instances each frame and calls
`apply_runtime_behavior_for_class(...)` for each registered behavior.

## TypeRenderer

Custom property editors can be registered via the `TypeRenderer` trait:

```rust
pub trait TypeRenderer: Send + Sync {
    fn can_render(&self, type_info: &RuntimeTypeInfo) -> bool;
    fn render(&self, ui_context: &mut dyn Any, value: &mut dyn Any,
              type_info: &RuntimeTypeInfo) -> RenderResult;
}
```

Registered with `inventory::submit!(TypeRendererRegistration::new(...))`.
The editor's property panel uses these to draw type-specific UI widgets
(e.g., a color picker for color types, a slider for float ranges).
