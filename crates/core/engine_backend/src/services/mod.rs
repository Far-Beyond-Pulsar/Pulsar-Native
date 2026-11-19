//! UI Services
//!
//! Background services and managers that support UI functionality:
//! - GPU renderer for 3D viewports, and the render runtimes that own and
//!   draw every one of them

pub mod core_project_builder;
#[cfg(feature = "render")]
pub mod device_loss;
#[cfg(feature = "render")]
pub mod gpu_renderer;
pub mod native_scripts;
#[cfg(feature = "pie")]
pub mod pie_blit;
#[cfg(feature = "pie")]
pub mod pie_host;
#[cfg(feature = "render")]
pub mod render_runtimes;

pub use core_project_builder::ensure_core_bootstrap;
#[cfg(feature = "render")]
pub use gpu_renderer::GpuRenderer;
#[cfg(feature = "pie")]
pub use pie_blit::PieBlit;
#[cfg(feature = "pie")]
pub use pie_host::PieHost;
