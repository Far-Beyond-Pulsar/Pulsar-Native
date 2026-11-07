//! `helio-web-demos` — every helio example as a WASM binary.
//!
//! Build a specific demo by passing its feature flag:
//!
//! ```sh
//! cargo build --lib -p helio-web-demos \
//!   --target wasm32-unknown-unknown \
//!   --no-default-features --features render_v2_basic
//! wasm-bindgen target/wasm32-unknown-unknown/release/helio_web_demos.wasm \
//!   --out-dir www/render_v2_basic --target web
//! ```
//!
//! Or run the `web` binary (`cargo run --bin web`) to build every demo at
//! once — interactively (TUI) or headless (`--headless`, used by CI). It also
//! writes the landing-page HTML and serves the result locally.

#[macro_use]
mod common;

// The published site's HTML (per-demo landing pages + master index) is written
// by the `web` binary's builder (src/bin/web.rs), which is the single source of
// truth — there is no build-script HTML generation.

// ── demo modules (each compiled only when its feature is active) ──────────────

#[cfg(feature = "render_v2_basic")]
#[path = "../examples-wasm/render_v2_basic.rs"]
mod render_v2_basic;

#[cfg(feature = "render_v2_sky")]
#[path = "../examples-wasm/render_v2_sky.rs"]
mod render_v2_sky;

#[cfg(feature = "debug_shapes")]
#[path = "../examples-wasm/debug_shapes.rs"]
mod debug_shapes;

#[cfg(feature = "indoor_room")]
#[path = "../examples-wasm/indoor_room.rs"]
mod indoor_room;

#[cfg(feature = "indoor_corridor")]
#[path = "../examples-wasm/indoor_corridor.rs"]
mod indoor_corridor;

#[cfg(feature = "outdoor_night")]
#[path = "../examples-wasm/outdoor_night.rs"]
mod outdoor_night;

#[cfg(feature = "outdoor_canyon")]
#[path = "../examples-wasm/outdoor_canyon.rs"]
mod outdoor_canyon;

#[cfg(feature = "hdr_demo")]
#[path = "../examples-wasm/hdr_demo.rs"]
mod hdr_demo;

#[cfg(feature = "color_grading_demo")]
#[path = "../examples-wasm/color_grading_demo.rs"]
mod color_grading_demo;

#[cfg(feature = "ies_demo")]
#[path = "../examples-wasm/ies_demo.rs"]
mod ies_demo;

#[cfg(feature = "indoor_cathedral")]
#[path = "../examples-wasm/indoor_cathedral.rs"]
mod indoor_cathedral;

#[cfg(feature = "indoor_server_room")]
#[path = "../examples-wasm/indoor_server_room.rs"]
mod indoor_server_room;

#[cfg(feature = "outdoor_city")]
#[path = "../examples-wasm/outdoor_city.rs"]
mod outdoor_city;

#[cfg(feature = "outdoor_volcano")]
#[path = "../examples-wasm/outdoor_volcano.rs"]
mod outdoor_volcano;

#[cfg(feature = "space_station")]
#[path = "../examples-wasm/space_station.rs"]
mod space_station;

#[cfg(feature = "light_benchmark")]
#[path = "../examples-wasm/light_benchmark.rs"]
mod light_benchmark;

#[cfg(feature = "hlfs_benchmark")]
#[path = "../examples-wasm/hlfs_benchmark.rs"]
mod hlfs_benchmark;

#[cfg(feature = "sdf_demo")]
#[path = "../examples-wasm/sdf_demo.rs"]
mod sdf_demo;

#[cfg(feature = "load_fbx")]
#[path = "../examples-wasm/load_fbx.rs"]
mod load_fbx;

#[cfg(feature = "load_fbx_embedded")]
#[path = "../examples-wasm/load_fbx_embedded.rs"]
mod load_fbx_embedded;

#[cfg(feature = "ship_flight")]
#[path = "../examples-wasm/ship_flight.rs"]
mod ship_flight;

#[cfg(feature = "simple_graph")]
#[path = "../examples-wasm/simple_graph.rs"]
mod simple_graph;

#[cfg(feature = "outdoor_rocks")]
#[path = "../examples-wasm/outdoor_rocks.rs"]
mod outdoor_rocks;

#[cfg(feature = "editor_demo")]
#[path = "../examples-wasm/editor_demo.rs"]
mod editor_demo;

#[cfg(feature = "vhs_backrooms")]
#[path = "../examples-wasm/vhs_backrooms.rs"]
mod vhs_backrooms;

// ── WASM entry point ──────────────────────────────────────────────────────────
// Exactly one feature is active per build; that branch calls `launch`.

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    #[cfg(feature = "render_v2_basic")]
    {
        helio_wasm::launch::<render_v2_basic::Demo>();
        return;
    }
    #[cfg(feature = "render_v2_sky")]
    {
        helio_wasm::launch::<render_v2_sky::Demo>();
        return;
    }
    #[cfg(feature = "debug_shapes")]
    {
        helio_wasm::launch::<debug_shapes::Demo>();
        return;
    }
    #[cfg(feature = "indoor_room")]
    {
        helio_wasm::launch::<indoor_room::Demo>();
        return;
    }
    #[cfg(feature = "indoor_corridor")]
    {
        helio_wasm::launch::<indoor_corridor::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_night")]
    {
        helio_wasm::launch::<outdoor_night::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_canyon")]
    {
        helio_wasm::launch::<outdoor_canyon::Demo>();
        return;
    }
    #[cfg(feature = "indoor_cathedral")]
    {
        helio_wasm::launch::<indoor_cathedral::Demo>();
        return;
    }
    #[cfg(feature = "indoor_server_room")]
    {
        helio_wasm::launch::<indoor_server_room::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_city")]
    {
        helio_wasm::launch::<outdoor_city::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_volcano")]
    {
        helio_wasm::launch::<outdoor_volcano::Demo>();
        return;
    }
    #[cfg(feature = "space_station")]
    {
        helio_wasm::launch::<space_station::Demo>();
        return;
    }
    #[cfg(feature = "light_benchmark")]
    {
        helio_wasm::launch::<light_benchmark::Demo>();
        return;
    }
    #[cfg(feature = "hlfs_benchmark")]
    {
        helio_wasm::launch::<hlfs_benchmark::Demo>();
        return;
    }
    #[cfg(feature = "sdf_demo")]
    {
        helio_wasm::launch::<sdf_demo::Demo>();
        return;
    }
    #[cfg(feature = "load_fbx")]
    {
        helio_wasm::launch::<load_fbx::Demo>();
        return;
    }
    #[cfg(feature = "load_fbx_embedded")]
    {
        helio_wasm::launch::<load_fbx_embedded::Demo>();
        return;
    }
    #[cfg(feature = "ship_flight")]
    {
        helio_wasm::launch::<ship_flight::Demo>();
        return;
    }
    #[cfg(feature = "simple_graph")]
    {
        helio_wasm::launch::<simple_graph::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_rocks")]
    {
        helio_wasm::launch::<outdoor_rocks::Demo>();
        return;
    }
    #[cfg(feature = "editor_demo")]
    {
        helio_wasm::launch::<editor_demo::Demo>();
        return;
    }
    #[cfg(feature = "vhs_backrooms")]
    {
        helio_wasm::launch::<vhs_backrooms::Demo>();
        return;
    }
    #[cfg(feature = "hdr_demo")]
    {
        helio_wasm::launch::<hdr_demo::Demo>();
        return;
    }
    #[cfg(feature = "color_grading_demo")]
    {
        helio_wasm::launch::<color_grading_demo::Demo>();
        return;
    }
    #[cfg(feature = "ies_demo")]
    {
        helio_wasm::launch::<ies_demo::Demo>();
        return;
    }
}

// ── Native entry (for `cargo run -p helio-web-demos --features <name>`) ───────

#[cfg(not(target_arch = "wasm32"))]
pub fn main() {
    #[cfg(feature = "render_v2_basic")]
    {
        helio_wasm::launch::<render_v2_basic::Demo>();
        return;
    }
    #[cfg(feature = "render_v2_sky")]
    {
        helio_wasm::launch::<render_v2_sky::Demo>();
        return;
    }
    #[cfg(feature = "debug_shapes")]
    {
        helio_wasm::launch::<debug_shapes::Demo>();
        return;
    }
    #[cfg(feature = "indoor_room")]
    {
        helio_wasm::launch::<indoor_room::Demo>();
        return;
    }
    #[cfg(feature = "indoor_corridor")]
    {
        helio_wasm::launch::<indoor_corridor::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_night")]
    {
        helio_wasm::launch::<outdoor_night::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_canyon")]
    {
        helio_wasm::launch::<outdoor_canyon::Demo>();
        return;
    }
    #[cfg(feature = "indoor_cathedral")]
    {
        helio_wasm::launch::<indoor_cathedral::Demo>();
        return;
    }
    #[cfg(feature = "indoor_server_room")]
    {
        helio_wasm::launch::<indoor_server_room::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_city")]
    {
        helio_wasm::launch::<outdoor_city::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_volcano")]
    {
        helio_wasm::launch::<outdoor_volcano::Demo>();
        return;
    }
    #[cfg(feature = "space_station")]
    {
        helio_wasm::launch::<space_station::Demo>();
        return;
    }
    #[cfg(feature = "light_benchmark")]
    {
        helio_wasm::launch::<light_benchmark::Demo>();
        return;
    }
    #[cfg(feature = "hlfs_benchmark")]
    {
        helio_wasm::launch::<hlfs_benchmark::Demo>();
        return;
    }
    #[cfg(feature = "sdf_demo")]
    {
        helio_wasm::launch::<sdf_demo::Demo>();
        return;
    }
    #[cfg(feature = "load_fbx")]
    {
        helio_wasm::launch::<load_fbx::Demo>();
        return;
    }
    #[cfg(feature = "load_fbx_embedded")]
    {
        helio_wasm::launch::<load_fbx_embedded::Demo>();
        return;
    }
    #[cfg(feature = "ship_flight")]
    {
        helio_wasm::launch::<ship_flight::Demo>();
        return;
    }
    #[cfg(feature = "simple_graph")]
    {
        helio_wasm::launch::<simple_graph::Demo>();
        return;
    }
    #[cfg(feature = "outdoor_rocks")]
    {
        helio_wasm::launch::<outdoor_rocks::Demo>();
        return;
    }
    #[cfg(feature = "editor_demo")]
    {
        helio_wasm::launch::<editor_demo::Demo>();
        return;
    }
    #[cfg(feature = "vhs_backrooms")]
    {
        helio_wasm::launch::<vhs_backrooms::Demo>();
        return;
    }
    #[cfg(feature = "hdr_demo")]
    {
        helio_wasm::launch::<hdr_demo::Demo>();
        return;
    }
    #[cfg(feature = "color_grading_demo")]
    {
        helio_wasm::launch::<color_grading_demo::Demo>();
        return;
    }
    #[cfg(feature = "ies_demo")]
    {
        helio_wasm::launch::<ies_demo::Demo>();
        return;
    }

    eprintln!("helio-web-demos: no feature selected. Pass --features <demo_name>.");
}
