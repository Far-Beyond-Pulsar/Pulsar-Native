use crates::{blueprint,exec_output};

// Context Ref:
//
// #Header (bold text in the engine)
// info here
//

///
/// String Node Value example
///

#[blueprint(type: crate::NodeTypes::fn_, category: "example string node",color="#0f009bff")]
pub fn example() -> String {
    "String Output!".to_string()
}

///
/// # Adds (A + B) / C
/// Adds Then Divides
///
#[blueprint(type: crate::NodeTypes::fn_, category: "example math node",color="#0f009bff")]
pub fn divide_add(add_a:f32,add_b:f32,div_c:f32) -> String {
    add_a+add_b/div_c
}