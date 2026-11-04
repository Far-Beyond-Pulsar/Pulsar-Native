#[cfg(target_os = "android")]
#[link(name = "c++_shared")]
extern "C" {}

#[cfg(feature = "editor_demo_mini")]
mod editor_demo_mini;

#[cfg(not(target_arch = "wasm32"))]
pub fn main() {
    #[cfg(feature = "editor_demo_mini")]
    {
        env_logger::try_init().ok();
        helio_wasm::launch::<editor_demo_mini::Demo>();
        return;
    }

    eprintln!("helio-android-demos: no feature selected. Use --features editor_demo_mini.");
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    #[cfg(feature = "editor_demo_mini")]
    {
        helio_wasm::launch_android::<editor_demo_mini::Demo>(app);
    }
}
