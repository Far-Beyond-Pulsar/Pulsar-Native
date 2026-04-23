use crate::{blueprint, exec_output, NodeTypes};
use rlua::{Lua, Result};

#[blueprint(type:NodeTypes::control_flow,category:"RLua (Experimental)",color="#003cff5d")]
pub fn runlua(code: String) -> String {
    let lua_runtime = Lua::new();
    let output: Result<String> = lua_runtime.load(code).eval();
    return output.unwrap();
}

#[blueprint(type:NodeTypes::pure,category:"RLua (Experimental)",color="#003cff5d")]
pub fn templateLua() -> String {
    return r#"
        local test = 20
        test*=2
        return test
    "#
    .to_string();
}
