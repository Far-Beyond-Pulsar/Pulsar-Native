//! Linux Vulkan Compositor Implementation
//!
//! Implements GPU composition using Vulkan with DMA-BUF based zero-copy texture sharing.
//!
//! ## Architecture
//!
//! ```text
//! ┌───────────────────────────────────────────┐
//! │     Vulkan Compositor Pipeline            │
//! ├───────────────────────────────────────────┤
//! │ 1. Acquire swapchain image                │
//! │ 2. Begin render pass (clear background)   │
//! │ 3. Draw Bevy texture (opaque)             │
//! │ 4. Draw GPUI texture (alpha-blended)      │
//! │ 5. End render pass                        │
//! │ 6. Queue present to X11/Wayland           │
//! └───────────────────────────────────────────┘
//! ```
//!
//! ## DMA-BUF Integration
//!
//! Linux uses the DMA-BUF mechanism for zero-copy texture sharing between processes
//! and GPU contexts. Both Bevy (via wgpu/Vulkan) and GPUI (via Blade/Vulkan) can
//! export textures as DMA-BUF file descriptors.
//!
//! ### Key Vulkan Extensions Used:
//! - `VK_KHR_external_memory_fd` - Import/export memory as file descriptors
//! - `VK_EXT_external_memory_dma_buf` - DMA-BUF specific support
//! - `VK_KHR_swapchain` - Present to X11/Wayland
//! - `VK_EXT_image_drm_format_modifier` - Handle DRM format modifiers

use super::{Compositor, CompositorState};
use anyhow::{bail, Context, Result};
use engine_backend::subsystems::render::NativeTextureHandle;
use gpui::SharedTextureHandle;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::ffi::{CStr, CString};
use std::sync::Arc;

// Note: ash is already available via GPUI's blade-graphics dependency
// We don't need to add it to Cargo.toml

/// Vulkan compositor for Linux
///
/// This compositor uses raw Vulkan API via the `ash` crate to compose
/// multiple GPU textures into a single window on Linux systems.
pub struct VulkanCompositor {
    /// Compositor state
    state: CompositorState,

    /// Vulkan instance
    instance: Option<ash::Instance>,

    /// Physical device (GPU)
    physical_device: ash::vk::PhysicalDevice,

    /// Logical device
    device: Option<ash::Device>,

    /// Graphics queue for rendering commands
    queue: ash::vk::Queue,

    /// Graphics queue family index
    queue_family_index: u32,

    /// Command pool for allocating command buffers
    command_pool: Option<ash::vk::CommandPool>,

    /// Command buffers (one per swapchain image)
    command_buffers: Vec<ash::vk::CommandBuffer>,

    /// Surface for window presentation
    surface: ash::vk::SurfaceKHR,

    /// Surface loader
    surface_loader: Option<ash::khr::surface::Instance>,

    /// Swapchain for presenting frames
    swapchain: ash::vk::SwapchainKHR,

    /// Swapchain loader
    swapchain_loader: Option<ash::khr::swapchain::Device>,

    /// Swapchain images
    swapchain_images: Vec<ash::vk::Image>,

    /// Swapchain image views
    swapchain_image_views: Vec<ash::vk::ImageView>,

    /// Swapchain framebuffers
    framebuffers: Vec<ash::vk::Framebuffer>,

    /// Render pass for composition
    render_pass: Option<ash::vk::RenderPass>,

    /// Pipeline for opaque rendering (Bevy layer)
    opaque_pipeline: Option<ash::vk::Pipeline>,

    /// Pipeline for alpha-blended rendering (GPUI layer)
    alpha_pipeline: Option<ash::vk::Pipeline>,

    /// Pipeline layout
    pipeline_layout: Option<ash::vk::PipelineLayout>,

    /// Descriptor set layout
    descriptor_set_layout: Option<ash::vk::DescriptorSetLayout>,

    /// Descriptor pool
    descriptor_pool: Option<ash::vk::DescriptorPool>,

    /// Vertex buffer (fullscreen quad)
    vertex_buffer: Option<ash::vk::Buffer>,

    /// Vertex buffer memory
    vertex_buffer_memory: Option<ash::vk::DeviceMemory>,

    /// Sampler for texture sampling
    sampler: Option<ash::vk::Sampler>,

    // === GPUI Texture State ===
    /// GPUI texture (imported from DMA-BUF)
    gpui_image: Option<ash::vk::Image>,

    /// GPUI texture memory
    gpui_memory: Option<ash::vk::DeviceMemory>,

    /// GPUI image view
    gpui_image_view: Option<ash::vk::ImageView>,

    /// GPUI descriptor set
    gpui_descriptor_set: Option<ash::vk::DescriptorSet>,

    /// Whether GPUI texture has been initialized
    gpui_initialized: bool,

    // === Bevy Texture State ===
    /// Bevy texture (imported from DMA-BUF or direct VkImage)
    bevy_image: Option<ash::vk::Image>,

    /// Bevy texture memory
    bevy_memory: Option<ash::vk::DeviceMemory>,

    /// Bevy image view
    bevy_image_view: Option<ash::vk::ImageView>,

    /// Bevy descriptor set
    bevy_descriptor_set: Option<ash::vk::DescriptorSet>,

    // === Synchronization ===
    /// Semaphore for image available
    image_available_semaphore: Option<ash::vk::Semaphore>,

    /// Semaphore for render finished
    render_finished_semaphore: Option<ash::vk::Semaphore>,

    /// Fence for CPU-GPU synchronization
    fence: Option<ash::vk::Fence>,

    /// Current frame index
    current_frame: usize,
}

impl VulkanCompositor {
    /// Create Vulkan instance with required extensions
    unsafe fn create_instance(
        entry: &ash::Entry,
        window: &(impl HasWindowHandle + HasDisplayHandle),
    ) -> Result<ash::Instance> {
        let app_info = ash::vk::ApplicationInfo::default()
            .application_name(CStr::from_bytes_with_nul_unchecked(b"Pulsar Engine\0"))
            .application_version(ash::vk::make_api_version(0, 0, 1, 0))
            .engine_name(CStr::from_bytes_with_nul_unchecked(b"Pulsar\0"))
            .engine_version(ash::vk::make_api_version(0, 0, 1, 0))
            .api_version(ash::vk::API_VERSION_1_2);

        // Get required extensions for window surface
        let display_handle = window.display_handle()?;
        let mut extension_names =
            ash_window::enumerate_required_extensions(display_handle.as_raw())?.to_vec();

        // Add external memory extensions for DMA-BUF support
        extension_names.push(ash::khr::get_physical_device_properties2::NAME.as_ptr());

        let create_info = ash::vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&extension_names);

        entry
            .create_instance(&create_info, None)
            .context("Failed to create Vulkan instance")
    }

    /// Select physical device (GPU)
    unsafe fn select_physical_device(
        instance: &ash::Instance,
    ) -> Result<(ash::vk::PhysicalDevice, u32)> {
        let devices = instance
            .enumerate_physical_devices()
            .context("Failed to enumerate physical devices")?;

        if devices.is_empty() {
            bail!("No Vulkan-capable GPU found");
        }

        // Find device with graphics queue
        for device in devices {
            let queue_families = instance.get_physical_device_queue_family_properties(device);

            for (index, family) in queue_families.iter().enumerate() {
                if family.queue_flags.contains(ash::vk::QueueFlags::GRAPHICS) {
                    tracing::info!("Selected Vulkan physical device with graphics queue");
                    return Ok((device, index as u32));
                }
            }
        }

        bail!("No suitable GPU with graphics queue found")
    }

    /// Create logical device with required extensions
    unsafe fn create_device(
        instance: &ash::Instance,
        physical_device: ash::vk::PhysicalDevice,
        queue_family_index: u32,
    ) -> Result<ash::Device> {
        let queue_priorities = [1.0];
        let queue_create_info = ash::vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priorities);

        // Enable required device extensions
        let device_extensions = [
            ash::khr::swapchain::NAME.as_ptr(),
            ash::khr::external_memory_fd::NAME.as_ptr(),
            ash::ext::external_memory_dma_buf::NAME.as_ptr(),
            ash::ext::image_drm_format_modifier::NAME.as_ptr(),
        ];

        let device_create_info = ash::vk::DeviceCreateInfo::default()
            .queue_create_infos(std::slice::from_ref(&queue_create_info))
            .enabled_extension_names(&device_extensions);

        instance
            .create_device(physical_device, &device_create_info, None)
            .context("Failed to create Vulkan logical device")
    }

    /// Create window surface
    unsafe fn create_surface(
        entry: &ash::Entry,
        instance: &ash::Instance,
        window: &(impl HasWindowHandle + HasDisplayHandle),
    ) -> Result<(ash::vk::SurfaceKHR, ash::khr::surface::Instance)> {
        let display_handle = window.display_handle()?;
        let window_handle = window.window_handle()?;
        let surface = ash_window::create_surface(
            entry,
            instance,
            display_handle.as_raw(),
            window_handle.as_raw(),
            None,
        )?;

        let surface_loader = ash::khr::surface::Instance::new(entry, instance);

        Ok((surface, surface_loader))
    }

    /// Create swapchain for presentation
    unsafe fn create_swapchain(
        instance: &ash::Instance,
        physical_device: ash::vk::PhysicalDevice,
        device: &ash::Device,
        surface: ash::vk::SurfaceKHR,
        surface_loader: &ash::khr::surface::Instance,
        width: u32,
        height: u32,
    ) -> Result<(
        ash::khr::swapchain::Device,
        ash::vk::SwapchainKHR,
        Vec<ash::vk::Image>,
    )> {
        let surface_capabilities =
            surface_loader.get_physical_device_surface_capabilities(physical_device, surface)?;

        let surface_format = surface_loader
            .get_physical_device_surface_formats(physical_device, surface)?
            .first()
            .copied()
            .unwrap_or(ash::vk::SurfaceFormatKHR {
                format: ash::vk::Format::B8G8R8A8_UNORM,
                color_space: ash::vk::ColorSpaceKHR::SRGB_NONLINEAR,
            });

        let present_mode = ash::vk::PresentModeKHR::FIFO; // VSync

        let extent = ash::vk::Extent2D { width, height };

        let image_count =
            (surface_capabilities.min_image_count + 1).min(surface_capabilities.max_image_count);

        let swapchain_create_info = ash::vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(surface_format.format)
            .image_color_space(surface_format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(ash::vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(ash::vk::SharingMode::EXCLUSIVE)
            .pre_transform(surface_capabilities.current_transform)
            .composite_alpha(ash::vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true);

        let swapchain_loader = ash::khr::swapchain::Device::new(instance, device);
        let swapchain = swapchain_loader.create_swapchain(&swapchain_create_info, None)?;
        let images = swapchain_loader.get_swapchain_images(swapchain)?;

        tracing::info!("Created Vulkan swapchain with {} images", images.len());

        Ok((swapchain_loader, swapchain, images))
    }

    /// Create render pass for composition
    unsafe fn create_render_pass(device: &ash::Device) -> Result<ash::vk::RenderPass> {
        let color_attachment = ash::vk::AttachmentDescription::default()
            .format(ash::vk::Format::B8G8R8A8_UNORM)
            .samples(ash::vk::SampleCountFlags::TYPE_1)
            .load_op(ash::vk::AttachmentLoadOp::CLEAR)
            .store_op(ash::vk::AttachmentStoreOp::STORE)
            .stencil_load_op(ash::vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(ash::vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(ash::vk::ImageLayout::UNDEFINED)
            .final_layout(ash::vk::ImageLayout::PRESENT_SRC_KHR);

        let color_attachment_ref = ash::vk::AttachmentReference::default()
            .attachment(0)
            .layout(ash::vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let subpass = ash::vk::SubpassDescription::default()
            .pipeline_bind_point(ash::vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_attachment_ref));

        let dependency = ash::vk::SubpassDependency::default()
            .src_subpass(ash::vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(ash::vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(ash::vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(ash::vk::AccessFlags::COLOR_ATTACHMENT_WRITE);

        let render_pass_info = ash::vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&color_attachment))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dependency));

        device
            .create_render_pass(&render_pass_info, None)
            .context("Failed to create render pass")
    }

    /// Stub for now - will be implemented when refactoring WindowState
    fn get_bevy_texture_handle(&self) -> Option<NativeTextureHandle> {
        // TODO: Get from window's Bevy renderer once WindowState is refactored
        None
    }

    /// Stub for now - will be implemented when refactoring WindowState
    fn get_gpui_texture_handle(&self) -> Option<SharedTextureHandle> {
        // TODO: Get from window's GPUI instance once WindowState is refactored
        None
    }
}

impl Compositor for VulkanCompositor {
    fn init(
        window: &(impl HasWindowHandle + HasDisplayHandle),
        width: u32,
        height: u32,
        scale_factor: f32,
    ) -> Result<Self> {
        tracing::info!(
            "🎨 Initializing Vulkan compositor: {}x{} @ {}x",
            width,
            height,
            scale_factor
        );

        unsafe {
            // Create Vulkan entry point
            let entry = ash::Entry::load()?;

            // Create instance
            let instance = Self::create_instance(&entry, window)?;

            // Select physical device
            let (physical_device, queue_family_index) = Self::select_physical_device(&instance)?;

            // Create logical device
            let device = Self::create_device(&instance, physical_device, queue_family_index)?;

            // Get graphics queue
            let queue = device.get_device_queue(queue_family_index, 0);

            // Create window surface
            let (surface, surface_loader) = Self::create_surface(&entry, &instance, window)?;

            // Create swapchain
            let (swapchain_loader, swapchain, swapchain_images) = Self::create_swapchain(
                &instance,
                physical_device,
                &device,
                surface,
                &surface_loader,
                width,
                height,
            )?;

            // Create render pass
            let render_pass = Self::create_render_pass(&device)?;

            // Create image views
            let swapchain_image_views: Vec<_> = swapchain_images
                .iter()
                .map(|&image| {
                    let view_info = ash::vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(ash::vk::ImageViewType::TYPE_2D)
                        .format(ash::vk::Format::B8G8R8A8_UNORM)
                        .subresource_range(ash::vk::ImageSubresourceRange {
                            aspect_mask: ash::vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 1,
                        });

                    device.create_image_view(&view_info, None)
                })
                .collect::<Result<Vec<_>, _>>()?;

            // Create framebuffers
            let framebuffers: Vec<_> = swapchain_image_views
                .iter()
                .map(|&view| {
                    let framebuffer_info = ash::vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass)
                        .attachments(std::slice::from_ref(&view))
                        .width(width)
                        .height(height)
                        .layers(1);

                    device.create_framebuffer(&framebuffer_info, None)
                })
                .collect::<Result<Vec<_>, _>>()?;

            // Create command pool
            let command_pool_info = ash::vk::CommandPoolCreateInfo::default()
                .queue_family_index(queue_family_index)
                .flags(ash::vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);

            let command_pool = device.create_command_pool(&command_pool_info, None)?;

            // Allocate command buffers
            let command_buffer_info = ash::vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(ash::vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(framebuffers.len() as u32);

            let command_buffers = device.allocate_command_buffers(&command_buffer_info)?;

            // Create synchronization objects
            let semaphore_info = ash::vk::SemaphoreCreateInfo::default();
            let fence_info =
                ash::vk::FenceCreateInfo::default().flags(ash::vk::FenceCreateFlags::SIGNALED);

            let image_available_semaphore = device.create_semaphore(&semaphore_info, None)?;
            let render_finished_semaphore = device.create_semaphore(&semaphore_info, None)?;
            let fence = device.create_fence(&fence_info, None)?;

            tracing::info!("✅ Vulkan compositor initialized successfully");

            Ok(Self {
                state: CompositorState {
                    width,
                    height,
                    scale_factor,
                    needs_render: true,
                },
                instance: Some(instance),
                physical_device,
                device: Some(device),
                queue,
                queue_family_index,
                command_pool: Some(command_pool),
                command_buffers,
                surface,
                surface_loader: Some(surface_loader),
                swapchain,
                swapchain_loader: Some(swapchain_loader),
                swapchain_images,
                swapchain_image_views,
                framebuffers,
                render_pass: Some(render_pass),
                opaque_pipeline: None,
                alpha_pipeline: None,
                pipeline_layout: None,
                descriptor_set_layout: None,
                descriptor_pool: None,
                vertex_buffer: None,
                vertex_buffer_memory: None,
                sampler: None,
                gpui_image: None,
                gpui_memory: None,
                gpui_image_view: None,
                gpui_descriptor_set: None,
                gpui_initialized: false,
                bevy_image: None,
                bevy_memory: None,
                bevy_image_view: None,
                bevy_descriptor_set: None,
                image_available_semaphore: Some(image_available_semaphore),
                render_finished_semaphore: Some(render_finished_semaphore),
                fence: Some(fence),
                current_frame: 0,
            })
        }
    }

    fn begin_frame(&mut self) -> Result<()> {
        // Acquire next swapchain image
        unsafe {
            let device = self.device.as_ref().context("Device not initialized")?;
            let fence = self.fence.context("Fence not initialized")?;

            // Wait for previous frame to finish
            device.wait_for_fences(&[fence], true, u64::MAX)?;
            device.reset_fences(&[fence])?;

            // Acquire image
            let (image_index, _) = self
                .swapchain_loader
                .as_ref()
                .context("Swapchain loader not initialized")?
                .acquire_next_image(
                    self.swapchain,
                    u64::MAX,
                    self.image_available_semaphore
                        .context("Semaphore not initialized")?,
                    ash::vk::Fence::null(),
                )?;

            self.current_frame = image_index as usize;
        }

        Ok(())
    }

    fn composite_bevy(&mut self, handle: &NativeTextureHandle) -> Result<Option<()>> {
        // TODO: Implement Bevy texture import from VkImage handle
        // This requires importing the VkImage from the handle or importing DMA-BUF

        match handle {
            NativeTextureHandle::Vulkan(_vk_image) => {
                tracing::debug!("Bevy Vulkan texture received (import not yet implemented)");
                Ok(Some(()))
            }
            _ => Ok(None),
        }
    }

    fn composite_gpui(&mut self, handle: &SharedTextureHandle, _should_render: bool) -> Result<()> {
        // TODO: Implement GPUI texture import from DMA-BUF
        // This requires using VK_KHR_external_memory_fd to import the FD

        match handle {
            SharedTextureHandle::DmaBuf { .. } => {
                tracing::debug!("GPUI DMA-BUF texture received (import not yet implemented)");
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn present(&mut self) -> Result<()> {
        unsafe {
            let device = self.device.as_ref().context("Device not initialized")?;
            let command_buffer = self.command_buffers[self.current_frame];

            // Begin command buffer
            let begin_info = ash::vk::CommandBufferBeginInfo::default();
            device.begin_command_buffer(command_buffer, &begin_info)?;

            // Begin render pass
            let clear_value = ash::vk::ClearValue {
                color: ash::vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 1.0],
                },
            };

            let render_pass_info = ash::vk::RenderPassBeginInfo::default()
                .render_pass(self.render_pass.context("Render pass not initialized")?)
                .framebuffer(self.framebuffers[self.current_frame])
                .render_area(ash::vk::Rect2D {
                    offset: ash::vk::Offset2D { x: 0, y: 0 },
                    extent: ash::vk::Extent2D {
                        width: self.state.width,
                        height: self.state.height,
                    },
                })
                .clear_values(std::slice::from_ref(&clear_value));

            device.cmd_begin_render_pass(
                command_buffer,
                &render_pass_info,
                ash::vk::SubpassContents::INLINE,
            );

            // TODO: Draw Bevy texture (opaque)
            // TODO: Draw GPUI texture (alpha-blended)

            device.cmd_end_render_pass(command_buffer);
            device.end_command_buffer(command_buffer)?;

            // Submit command buffer
            let wait_semaphores = [self
                .image_available_semaphore
                .context("Semaphore not initialized")?];
            let wait_stages = [ash::vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let signal_semaphores = [self
                .render_finished_semaphore
                .context("Semaphore not initialized")?];

            let submit_info = ash::vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(std::slice::from_ref(&command_buffer))
                .signal_semaphores(&signal_semaphores);

            device.queue_submit(
                self.queue,
                &[submit_info],
                self.fence.context("Fence not initialized")?,
            )?;

            // Present
            let image_index = self.current_frame as u32;
            let present_info = ash::vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(std::slice::from_ref(&self.swapchain))
                .image_indices(std::slice::from_ref(&image_index));

            self.swapchain_loader
                .as_ref()
                .context("Swapchain loader not initialized")?
                .queue_present(self.queue, &present_info)?;
        }

        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        tracing::info!("🔄 Resizing Vulkan compositor to {}x{}", width, height);

        // TODO: Implement swapchain recreation
        // - Wait for device idle
        // - Destroy old swapchain, framebuffers, image views
        // - Create new swapchain with new dimensions
        // - Recreate framebuffers

        self.state.width = width;
        self.state.height = height;

        Ok(())
    }

    fn state(&self) -> &CompositorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut CompositorState {
        &mut self.state
    }
}

impl Drop for VulkanCompositor {
    fn drop(&mut self) {
        unsafe {
            if let Some(device) = &self.device {
                // Wait for all operations to complete
                let _ = device.device_wait_idle();

                // Destroy synchronization objects
                if let Some(sem) = self.image_available_semaphore {
                    device.destroy_semaphore(sem, None);
                }
                if let Some(sem) = self.render_finished_semaphore {
                    device.destroy_semaphore(sem, None);
                }
                if let Some(fence) = self.fence {
                    device.destroy_fence(fence, None);
                }

                // Destroy framebuffers
                for fb in &self.framebuffers {
                    device.destroy_framebuffer(*fb, None);
                }

                // Destroy image views
                for view in &self.swapchain_image_views {
                    device.destroy_image_view(*view, None);
                }

                // Destroy render pass
                if let Some(rp) = self.render_pass {
                    device.destroy_render_pass(rp, None);
                }

                // Free command buffers
                if let Some(pool) = self.command_pool {
                    device.free_command_buffers(pool, &self.command_buffers);
                    device.destroy_command_pool(pool, None);
                }

                // Destroy swapchain
                if let Some(loader) = &self.swapchain_loader {
                    loader.destroy_swapchain(self.swapchain, None);
                }

                // Destroy surface
                if let Some(loader) = &self.surface_loader {
                    loader.destroy_surface(self.surface, None);
                }

                // Destroy device
                device.destroy_device(None);
            }

            // Destroy instance
            if let Some(instance) = &self.instance {
                instance.destroy_instance(None);
            }

            tracing::info!("🧹 Vulkan compositor destroyed");
        }
    }
}
