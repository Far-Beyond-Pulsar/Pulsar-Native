//! Driver statistics of the planet's compute pipelines: registers, spills,
//! shared memory, as the Vulkan driver compiles them
//! (`VK_KHR_pipeline_executable_properties`).
//!
//! `cargo run --release -p helio-pass-voxel-planet --example shader_stats [entry]`
//!
//! Registers bound occupancy, and occupancy bounds how much memory latency a
//! traversal loop hides; no profiler reports this for Vulkan compute on this
//! machine. Sources are the Earth program's (`engine::validation_sources`),
//! or the WGSL files in `SHADER_STATS_FILES` (comma-separated, Helio's
//! `//!use` prelude resolved: any pass's compute shader, or variants of one
//! to compare; binary size / 16 is about the instruction count), compiled
//! with naga as wgpu does (index accesses restricted, buffer accesses left
//! to the driver's robustness).

use ash::vk;
use std::ffi::CStr;

fn main() {
    let filter = std::env::args().nth(1);
    let sources: Vec<(String, String)> = match std::env::var("SHADER_STATS_FILES") {
        Ok(files) => files
            .split(',')
            .map(|f| {
                let name = std::path::Path::new(f)
                    .file_name()
                    .map_or(f.to_string(), |n| n.to_string_lossy().into_owned());
                (
                    name,
                    helio_core::shader::resolve(&std::fs::read_to_string(f).expect("a WGSL file"))
                        .into_owned(),
                )
            })
            .collect(),
        Err(_) => helio_pass_voxel_planet::engine::validation_sources(),
    };
    unsafe {
        let entry = ash::Entry::load().expect("Vulkan loader");
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
        let instance = entry
            .create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
            .expect("instance");
        let pdev = instance
            .enumerate_physical_devices()
            .expect("devices")
            .into_iter()
            .find(|p| {
                instance.get_physical_device_properties(*p).device_type
                    == vk::PhysicalDeviceType::DISCRETE_GPU
            })
            .expect("a discrete GPU");
        let name = instance.get_physical_device_properties(pdev);
        println!(
            "device {:?}",
            name.device_name_as_c_str().unwrap_or_default()
        );
        let family = instance
            .get_physical_device_queue_family_properties(pdev)
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::COMPUTE))
            .expect("compute queue") as u32;
        // Every supported core feature, as wgpu enables what it can.
        let mut f11 = vk::PhysicalDeviceVulkan11Features::default();
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut query = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut f11)
            .push_next(&mut f12)
            .push_next(&mut f13);
        instance.get_physical_device_features2(pdev, &mut query);
        let core = query.features;
        f11.p_next = std::ptr::null_mut();
        f12.p_next = std::ptr::null_mut();
        f13.p_next = std::ptr::null_mut();
        let mut exec = vk::PhysicalDevicePipelineExecutablePropertiesFeaturesKHR::default()
            .pipeline_executable_info(true);
        let mut features = vk::PhysicalDeviceFeatures2::default()
            .features(core)
            .push_next(&mut f11)
            .push_next(&mut f12)
            .push_next(&mut f13)
            .push_next(&mut exec);
        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let extensions = [ash::khr::pipeline_executable_properties::NAME.as_ptr()];
        let device = instance
            .create_device(
                pdev,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(&queues)
                    .enabled_extension_names(&extensions)
                    .push_next(&mut features),
                None,
            )
            .expect("device");
        let stats_api = ash::khr::pipeline_executable_properties::Device::new(&instance, &device);

        for (label, source) in &sources {
            let module = match naga::front::wgsl::parse_str(source) {
                Ok(m) => m,
                Err(e) => panic!("{label}: {}", e.emit_to_string(source)),
            };
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{label}: {e:?}"));
            // Descriptor set layouts from every bound global.
            let mut sets: Vec<Vec<vk::DescriptorSetLayoutBinding>> = Vec::new();
            for (_, var) in module.global_variables.iter() {
                let Some(binding) = &var.binding else {
                    continue;
                };
                let kind = match var.space {
                    naga::AddressSpace::Uniform => vk::DescriptorType::UNIFORM_BUFFER,
                    naga::AddressSpace::Storage { .. } => vk::DescriptorType::STORAGE_BUFFER,
                    naga::AddressSpace::Handle => match module.types[var.ty].inner {
                        naga::TypeInner::Image {
                            class: naga::ImageClass::Storage { .. },
                            ..
                        } => vk::DescriptorType::STORAGE_IMAGE,
                        naga::TypeInner::Image { .. } => vk::DescriptorType::SAMPLED_IMAGE,
                        naga::TypeInner::Sampler { .. } => vk::DescriptorType::SAMPLER,
                        _ => continue,
                    },
                    _ => continue,
                };
                let group = binding.group as usize;
                if sets.len() <= group {
                    sets.resize(group + 1, Vec::new());
                }
                sets[group].push(
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(binding.binding)
                        .descriptor_type(kind)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::COMPUTE),
                );
            }
            let set_layouts: Vec<vk::DescriptorSetLayout> = sets
                .iter()
                .map(|b| {
                    device
                        .create_descriptor_set_layout(
                            &vk::DescriptorSetLayoutCreateInfo::default().bindings(b),
                            None,
                        )
                        .unwrap()
                })
                .collect();
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
                    None,
                )
                .unwrap();
            for ep in &module.entry_points {
                if ep.stage != naga::ShaderStage::Compute
                    || filter.as_deref().is_some_and(|f| !ep.name.contains(f))
                {
                    continue;
                }
                let options = naga::back::spv::Options {
                    lang_version: (1, 3),
                    bounds_check_policies: naga::proc::BoundsCheckPolicies {
                        index: naga::proc::BoundsCheckPolicy::Restrict,
                        buffer: naga::proc::BoundsCheckPolicy::Unchecked,
                        image_load: naga::proc::BoundsCheckPolicy::Restrict,
                        binding_array: naga::proc::BoundsCheckPolicy::Unchecked,
                    },
                    ..Default::default()
                };
                let pipeline_options = naga::back::spv::PipelineOptions {
                    shader_stage: naga::ShaderStage::Compute,
                    entry_point: ep.name.clone(),
                };
                // Overrides resolved for the entry point, as wgpu does.
                let (resolved, resolved_info) = naga::back::pipeline_constants::process_overrides(
                    &module,
                    &info,
                    Some((naga::ShaderStage::Compute, ep.name.as_str())),
                    &Default::default(),
                )
                .unwrap_or_else(|e| panic!("{label} {}: {e:?}", ep.name));
                let words = naga::back::spv::write_vec(
                    &resolved,
                    &resolved_info,
                    &options,
                    Some(&pipeline_options),
                )
                .unwrap();
                let shader = device
                    .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
                    .unwrap();
                let entry_name = std::ffi::CString::new(ep.name.as_str()).unwrap();
                let stage = vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::COMPUTE)
                    .module(shader)
                    .name(&entry_name);
                let create = vk::ComputePipelineCreateInfo::default()
                    .stage(stage)
                    .layout(layout)
                    .flags(vk::PipelineCreateFlags::CAPTURE_STATISTICS_KHR);
                let started = std::time::Instant::now();
                let pipeline = match device.create_compute_pipelines(
                    vk::PipelineCache::null(),
                    &[create],
                    None,
                ) {
                    Ok(p) => p[0],
                    Err((_, e)) => {
                        println!("{label} {}: pipeline failed {e:?}", ep.name);
                        continue;
                    }
                };
                let executables = stats_api
                    .get_pipeline_executable_properties(
                        &vk::PipelineInfoKHR::default().pipeline(pipeline),
                    )
                    .unwrap();
                for index in 0..executables.len() as u32 {
                    let stats = stats_api
                        .get_pipeline_executable_statistics(
                            &vk::PipelineExecutableInfoKHR::default()
                                .pipeline(pipeline)
                                .executable_index(index),
                        )
                        .unwrap();
                    let mut line = format!(
                        "{label:24} {:24} {:8.1} ms",
                        ep.name,
                        started.elapsed().as_secs_f64() * 1e3
                    );
                    for s in &stats {
                        let name = CStr::from_ptr(s.name.as_ptr()).to_string_lossy();
                        let value = match s.format {
                            vk::PipelineExecutableStatisticFormatKHR::BOOL32 => {
                                format!("{}", s.value.b32)
                            }
                            vk::PipelineExecutableStatisticFormatKHR::INT64 => {
                                format!("{}", s.value.i64)
                            }
                            vk::PipelineExecutableStatisticFormatKHR::UINT64 => {
                                format!("{}", s.value.u64)
                            }
                            _ => format!("{:.2}", s.value.f64),
                        };
                        line.push_str(&format!(" | {name} {value}"));
                    }
                    println!("{line}");
                }
                device.destroy_pipeline(pipeline, None);
                device.destroy_shader_module(shader, None);
            }
            device.destroy_pipeline_layout(layout, None);
            for s in set_layouts {
                device.destroy_descriptor_set_layout(s, None);
            }
        }
        device.destroy_device(None);
        instance.destroy_instance(None);
    }
}
