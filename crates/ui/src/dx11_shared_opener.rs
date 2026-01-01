///! DX11 Shared Resource Opener
///! 
///! Opens DX12 shared NT handles in DX11 and creates SRVs for GPUI rendering
///! This is the critical bridge between Bevy's DX12 renderer and GPUI's DX11 renderer

use std::sync::{Arc, Mutex, OnceLock};
use anyhow::{Context, Result};

#[cfg(target_os = "windows")]
pub mod windows_impl {
    use super::*;
    use windows::Win32::Graphics::{
        Direct3D11::*,
        Direct3D::*,
        Dxgi::Common::*,
        Dxgi::*,
    };
    use windows::Win32::Foundation::HANDLE;
    use windows::core::Interface;
    use std::collections::HashMap;

    /// Manages opened shared textures and their SRVs
    /// This keeps the D3D11 textures and SRVs alive for the lifetime of the viewport
    #[derive(Debug)]
    pub struct SharedTextureManager {
        device: ID3D11Device,
        /// Map from NT handle value to (texture, SRV)
        opened_textures: HashMap<usize, (ID3D11Texture2D, ID3D11ShaderResourceView)>,
    }

    impl SharedTextureManager {
        /// Create a new manager with the given D3D11 device
        pub unsafe fn new(device: ID3D11Device) -> Self {
            tracing::debug!("[DX11-OPENER] 🔧 Creating SharedTextureManager");
            Self {
                device,
                opened_textures: HashMap::new(),
            }
        }

        /// Open a DX12 shared NT handle in DX11 and create an SRV
        /// Returns a pointer to the SRV that can be passed to GPUI
        pub unsafe fn open_and_create_srv(
            &mut self,
            nt_handle: usize,
            width: u32,
            height: u32,
        ) -> Result<*mut std::ffi::c_void> {
            // Check if we already opened this handle
            if let Some((_, srv)) = self.opened_textures.get(&nt_handle) {
                tracing::debug!("[DX11-OPENER] ♻️ Reusing existing SRV for handle 0x{:X}", nt_handle);
                let srv_ptr = srv.as_raw() as *mut std::ffi::c_void;
                return Ok(srv_ptr);
            }

            tracing::debug!("[DX11-OPENER] 📂 Opening shared NT handle 0x{:X} in DX11...", nt_handle);

            // Step 1: Open the shared resource in DX11
            // Convert usize to HANDLE (raw pointer)
            let handle = HANDLE(nt_handle as *mut std::ffi::c_void);
            
            // We need ID3D11Device1::OpenSharedResource1 for NT handles!
            // First, try to cast device to ID3D11Device1
            let device1: ID3D11Device1 = match self.device.cast() {
                Ok(dev) => {
                    tracing::debug!("[DX11-OPENER] ✅ Successfully cast to ID3D11Device1");
                    dev
                }
                Err(e) => {
                    tracing::error!("[DX11-OPENER] ❌ Failed to cast to ID3D11Device1: {:?}", e);
                    tracing::error!("[DX11-OPENER] Device doesn't support D3D11.1!");
                    return Err(anyhow::anyhow!("Device doesn't support ID3D11Device1"));
                }
            };

            // OpenSharedResource1: Takes HANDLE, returns Result<T> where T is inferred
            // API: pub unsafe fn OpenSharedResource1<P0, T>(&self, hresource: P0) -> windows_core::Result<T>
            let texture: ID3D11Texture2D = match device1.OpenSharedResource1(handle) {
                Ok(tex) => {
                    tracing::info!("[DX11-OPENER] ✅ OpenSharedResource1 succeeded!");
                    tex
                }
                Err(e) => {
                    tracing::error!("[DX11-OPENER] ❌ OpenSharedResource1 FAILED!");
                    tracing::error!("[DX11-OPENER] 🔍 HRESULT: 0x{:08X}", e.code().0);
                    tracing::error!("[DX11-OPENER] 📋 Error: {:?}", e);
                    tracing::info!("[DX11-OPENER] 💡 Common causes:");
                    tracing::info!("[DX11-OPENER]    - Different GPU adapters (DX12 vs DX11)");
                    tracing::info!("[DX11-OPENER]    - Handle already closed/invalid");
                    tracing::info!("[DX11-OPENER]    - Missing D3D11_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS");
                    return Err(e).context("Failed to open shared resource with OpenSharedResource1");
                }
            };
            tracing::debug!("[DX11-OPENER] ✅ Opened D3D11 texture from shared handle");

            // Step 2: Create Shader Resource View
            let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                    },
                },
            };

            let mut srv: Option<ID3D11ShaderResourceView> = None;
            self.device.CreateShaderResourceView(
                &texture,
                Some(&srv_desc),
                Some(&mut srv),
            ).context("Failed to create SRV")?;

            let srv = srv.context("SRV was None after creation")?;
            tracing::debug!("[DX11-OPENER] ✅ Created SRV for {}x{} texture", width, height);

            // Get raw pointer before storing
            let srv_ptr = srv.as_raw() as *mut std::ffi::c_void;

            // Store for later reuse (keeps COM objects alive)
            self.opened_textures.insert(nt_handle, (texture, srv));

            tracing::debug!("[DX11-OPENER] 🎉 SRV ready at ptr: {:p}", srv_ptr);
            Ok(srv_ptr)
        }

        /// Close and release a shared texture
        pub fn release_texture(&mut self, nt_handle: usize) {
            if self.opened_textures.remove(&nt_handle).is_some() {
                tracing::debug!("[DX11-OPENER] 🗑️ Released texture for handle 0x{:X}", nt_handle);
            }
        }

        /// Get the D3D11 device
        pub fn device(&self) -> &ID3D11Device {
            &self.device
        }
    }

    impl Drop for SharedTextureManager {
        fn drop(&mut self) {
            tracing::debug!("[DX11-OPENER] 🧹 Dropping SharedTextureManager, releasing {} textures", 
                self.opened_textures.len());
            self.opened_textures.clear();
        }
    }

    /// Global singleton manager
    static SHARED_TEXTURE_MANAGER: OnceLock<Arc<Mutex<SharedTextureManager>>> = OnceLock::new();

    /// Initialize the global manager with a D3D11 device
    pub unsafe fn init_manager(device: ID3D11Device) {
        tracing::debug!("[DX11-OPENER] 🚀 Initializing global SharedTextureManager");
        let manager = SharedTextureManager::new(device);
        SHARED_TEXTURE_MANAGER.set(Arc::new(Mutex::new(manager)))
            .expect("SharedTextureManager already initialized");
    }

    /// Get the global manager
    pub fn get_manager() -> Option<Arc<Mutex<SharedTextureManager>>> {
        SHARED_TEXTURE_MANAGER.get().cloned()
    }

    /// Helper: Open a shared NT handle and get an SRV pointer for GPUI
    /// If manager is not initialized, attempts lazy initialization by getting device from GPUI window
    pub unsafe fn open_shared_handle_for_gpui(
        nt_handle: usize,
        width: u32,
        height: u32,
    ) -> Result<*mut std::ffi::c_void> {
        // Try to get existing manager
        if let Some(manager) = get_manager() {
            let mut mg = manager.lock()
                .map_err(|e| anyhow::anyhow!("Failed to lock manager: {}", e))?;
            return mg.open_and_create_srv(nt_handle, width, height);
        }

        // Manager not initialized - try to initialize it lazily
        // We need to get the D3D11 device from GPUI somehow...
        // For now, return an error with instructions
        anyhow::bail!(
            "SharedTextureManager not initialized! \n\
             This means we don't have access to GPUI's D3D11 device yet.\n\
             The device is created when the first window opens.\n\
             \n\
             SOLUTION: We need to expose the D3D11 device from GPUI or initialize \n\
             the manager after the first window is created."
        )
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::*;

// Helper to get D3D11 device from GPUI's DirectXRenderer
// This needs to be called from the engine after GPUI window is created
#[cfg(target_os = "windows")]
pub unsafe fn init_from_gpui_window() -> anyhow::Result<()> {
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::core::Interface;

    tracing::debug!("[DX11-OPENER] 🔍 Attempting to initialize from current GPUI state...");
    
    // Try to get the DXGI device from the current process
    // This is a workaround - ideally GPUI would expose its device
    let dxgi_factory: IDXGIFactory1 = CreateDXGIFactory1()?;
    let adapter: IDXGIAdapter1 = dxgi_factory.EnumAdapters1(0)?;
    
    // Create our own D3D11 device (it will share GPU resources)
    let mut device: Option<ID3D11Device> = None;
    let mut _device_context: Option<ID3D11DeviceContext> = None;
    
    // Use the actual enum values instead of constants
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL;
    let feature_levels = [
        D3D_FEATURE_LEVEL(0xb100), // D3D_FEATURE_LEVEL_11_1
        D3D_FEATURE_LEVEL(0xb000), // D3D_FEATURE_LEVEL_11_0
    ];
    
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE;
    
    D3D11CreateDevice(
        &adapter,
        D3D_DRIVER_TYPE(0), // D3D_DRIVER_TYPE_UNKNOWN
        None,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        Some(&feature_levels),
        D3D11_SDK_VERSION,
        Some(&mut device),
        None,
        Some(&mut _device_context),
    )?;
    
    let device = device.context("Failed to create D3D11 device")?;
    tracing::info!("[DX11-OPENER] ✅ Created D3D11 device for shared resource opening");
    
    init_manager(device);
    tracing::info!("[DX11-OPENER] ✅ Manager initialized successfully");
    
    Ok(())
}
