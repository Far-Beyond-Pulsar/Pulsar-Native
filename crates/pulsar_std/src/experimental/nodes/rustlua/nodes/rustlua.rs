use crate::{blueprint, exec_output, NodeTypes};
use rlua::{Lua, Result, Table};

/// Create a sandboxed Lua runtime with dangerous operations removed.
/// This prevents blueprints from executing shell commands or accessing
/// the filesystem through Lua.
fn create_sandboxed_lua() -> Lua {
    let lua = Lua::new();

    // Remove dangerous globals to sandbox the Lua environment.
    // Users can still run math, string, table operations, etc.
    lua.context(|ctx| -> Result<()> {
        let globals: Table = ctx.globals();

        // Block OS-level access.
        globals.set("os", ctx.create_table()?)?;
        let os_table: Table = globals.get("os")?;
        // Only allow os.time() and os.date() — block os.execute(), os.rename(), os.exit(), etc.
        let safe_os = ctx.create_table()?;
        safe_os.set("time", os_table.get::<_, rlua::Function>("time").ok())?;
        safe_os.set("date", os_table.get::<_, rlua::Function>("date").ok())?;
        globals.set("os", safe_os)?;

        // Block filesystem I/O.
        globals.set("io", rlua::Nil)?;

        // Block debug library (can be used to bypass sandbox).
        globals.set("debug", rlua::Nil)?;

        // Block require/package (can be used to load native modules).
        globals.set("require", rlua::Nil)?;
        globals.set("package", rlua::Nil)?;
        globals.set("dofile", rlua::Nil)?;
        globals.set("loadfile", rlua::Nil)?;

        // Block raw access to metatables on globals (can be used to escape).
        // Keep setmetatable/getmetatable for normal tables, but block __gc and __mode abuse.
        globals.set("collectgarbage", rlua::Nil)?;

        Ok(())
    })
    .expect("Failed to configure Lua sandbox");

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
