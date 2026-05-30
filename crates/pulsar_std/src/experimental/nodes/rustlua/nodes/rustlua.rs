use crate::{blueprint, exec_output, NodeTypes};
use rlua::{Lua, Result, Table};

fn create_sandboxed_lua() -> Lua {
    let lua = Lua::new();

    let globals = lua.globals().expect("Failed to get Lua globals");

    let os_table: Table = globals
        .get("os")
        .unwrap_or_else(|_| lua.create_table().expect("Failed to create table"));
    let safe_os = lua.create_table().expect("Failed to create table");
    if let Ok(time_fn) = os_table.get::<_, rlua::Function>("time") {
        safe_os.set("time", time_fn).ok();
    }
    if let Ok(date_fn) = os_table.get::<_, rlua::Function>("date") {
        safe_os.set("date", date_fn).ok();
    }
    globals.set("os", safe_os).expect("Failed to set os");

    globals.set("io", ()).expect("Failed to block io");
    globals.set("debug", ()).expect("Failed to block debug");
    globals.set("require", ()).expect("Failed to block require");
    globals.set("package", ()).expect("Failed to block package");
    globals.set("dofile", ()).expect("Failed to block dofile");
    globals.set("loadfile", ()).expect("Failed to block loadfile");
    globals.set("collectgarbage", ()).expect("Failed to block collectgarbage");

    lua
}
        if let Ok(date_func) = os_table.get::<_, rlua::Function>("date") {
            safe_os.set("date", date_func)?;
        }
        globals.set("os", safe_os)?;
        globals.set("io", rlua::Nil)?;
        globals.set("debug", rlua::Nil)?;
        globals.set("require", rlua::Nil)?;
        globals.set("package", rlua::Nil)?;
        globals.set("dofile", rlua::Nil)?;
        globals.set("loadfile", rlua::Nil)?;
        globals.set("collectgarbage", rlua::Nil)?;
        Ok(())
    }

    configure(&lua).expect("Failed to configure Lua sandbox");

    lua
}

#[blueprint(type:NodeTypes::control_flow,category:"RLua (Experimental)",color="#003cff5d")]
pub fn runlua(code: String) -> String {
    let lua_runtime = create_sandboxed_lua();
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
