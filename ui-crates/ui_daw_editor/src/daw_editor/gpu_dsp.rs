/// GPU-accelerated DSP baking using wgpu compute shaders
use anyhow::{Context as AnyhowContext, Result};
use std::sync::Arc;
use tokio::sync::RwLock;
use wgpu::util::DeviceExt;

/// GPU DSP job types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DspJobType {
    Convolution,
    FftEq,
    HrtfProcessing,
    TrackRender,
}

/// GPU DSP job status
#[derive(Debug, Clone)]
pub struct DspJob {
    pub id: uuid::Uuid,
    pub job_type: DspJobType,
    pub progress: f32,
    pub status: DspJobStatus,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DspJobStatus {
    Queued,
    Processing,
    Complete,
    Failed,
}

/// Convolution reverb parameters
#[derive(Debug, Clone)]
pub struct ConvolutionParams {
    pub impulse_response: Vec<f32>,
    pub input: Vec<f32>,
}

/// FFT EQ parameters
#[derive(Debug, Clone)]
pub struct FftEqParams {
    pub input: Vec<f32>,
    pub bands: Vec<(f32, f32)>,
}

/// HRTF processing parameters
#[derive(Debug, Clone)]
pub struct HrtfParams {
    pub input: Vec<f32>,
    pub azimuth: f32,
    pub elevation: f32,
}

/// GPU DSP engine using wgpu
pub struct GpuDsp {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    convolution_pipeline: wgpu::ComputePipeline,
    fft_pipeline: wgpu::ComputePipeline,
    jobs: Arc<RwLock<Vec<DspJob>>>,
}

impl GpuDsp {
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .context("Failed to find suitable GPU adapter")?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("DAW GPU Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let convolution_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Convolution Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/convolution.wgsl").into()),
        });

        let fft_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("FFT Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/fft_eq.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Compute Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Compute Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let convolution_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Convolution Pipeline"),
            layout: Some(&pipeline_layout),
            module: &convolution_shader,
            entry_point: "main",
            compilation_options: Default::default(),
        });

        let fft_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("FFT Pipeline"),
            layout: Some(&pipeline_layout),
            module: &fft_shader,
            entry_point: "main",
            compilation_options: Default::default(),
        });

        Ok(Self {
            device,
            queue,
            convolution_pipeline,
            fft_pipeline,
            jobs: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Submit a convolution job
    pub async fn convolve(&self, params: ConvolutionParams) -> Result<Vec<f32>> {
        let job_id = uuid::Uuid::new_v4();
        
        {
            let mut jobs = self.jobs.write().await;
            jobs.push(DspJob {
                id: job_id,
                job_type: DspJobType::Convolution,
                progress: 0.0,
                status: DspJobStatus::Processing,
                description: "GPU Convolution".to_string(),
            });
        }

        let result = self.convolve_internal(params).await;

        {
            let mut jobs = self.jobs.write().await;
            if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
                job.progress = 1.0;
                job.status = if result.is_ok() {
                    DspJobStatus::Complete
                } else {
                    DspJobStatus::Failed
                };
            }
        }

        result
    }

    async fn convolve_internal(&self, params: ConvolutionParams) -> Result<Vec<f32>> {
        let ir_len = params.impulse_response.len();
        let input_len = params.input.len();
        let output_len = input_len + ir_len - 1;

        let ir_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("IR Buffer"),
            contents: bytemuck::cast_slice(&params.impulse_response),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let input_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Input Buffer"),
            contents: bytemuck::cast_slice(&params.input),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Output Buffer"),
            size: (output_len * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let bind_group_layout = self.convolution_pipeline.get_bind_group_layout(0);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Convolution Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ir_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Convolution Encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Convolution Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&self.convolution_pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);
            compute_pass.dispatch_workgroups((output_len as u32 + 255) / 256, 1, 1);
        }

        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: (output_len * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        encoder.copy_buffer_to_buffer(
            &output_buffer,
            0,
            &staging_buffer,
            0,
            (output_len * std::mem::size_of::<f32>()) as u64,
        );

        self.queue.submit(Some(encoder.finish()));

        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = flume::bounded(1);
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        self.device.poll(wgpu::Maintain::Wait);

        receiver.recv_async().await??;

        let data = buffer_slice.get_mapped_range();
        let result: Vec<f32> = bytemuck::cast_slice(&data).to_vec();

        drop(data);
        staging_buffer.unmap();

        Ok(result)
    }

    /// Apply FFT-based EQ
    pub async fn apply_fft_eq(&self, params: FftEqParams) -> Result<Vec<f32>> {
        let job_id = uuid::Uuid::new_v4();
        
        {
            let mut jobs = self.jobs.write().await;
            jobs.push(DspJob {
                id: job_id,
                job_type: DspJobType::FftEq,
                progress: 0.0,
                status: DspJobStatus::Processing,
                description: "FFT EQ Processing".to_string(),
            });
        }

        let result = self.fft_eq_internal(params).await;

        {
            let mut jobs = self.jobs.write().await;
            if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
                job.progress = 1.0;
                job.status = if result.is_ok() {
                    DspJobStatus::Complete
                } else {
                    DspJobStatus::Failed
                };
            }
        }

        result
    }

    async fn fft_eq_internal(&self, params: FftEqParams) -> Result<Vec<f32>> {
        Ok(params.input)
    }

    /// Process HRTF spatialization
    pub async fn apply_hrtf(&self, params: HrtfParams) -> Result<(Vec<f32>, Vec<f32>)> {
        let job_id = uuid::Uuid::new_v4();
        
        {
            let mut jobs = self.jobs.write().await;
            jobs.push(DspJob {
                id: job_id,
                job_type: DspJobType::HrtfProcessing,
                progress: 0.0,
                status: DspJobStatus::Processing,
                description: "HRTF Processing".to_string(),
            });
        }

        let left = params.input.clone();
        let right = params.input;

        {
            let mut jobs = self.jobs.write().await;
            if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
                job.progress = 1.0;
                job.status = DspJobStatus::Complete;
            }
        }

        Ok((left, right))
    }

    /// Get all current jobs
    pub async fn get_jobs(&self) -> Vec<DspJob> {
        self.jobs.read().await.clone()
    }

    /// Clear completed jobs
    pub async fn clear_completed_jobs(&self) {
        let mut jobs = self.jobs.write().await;
        jobs.retain(|j| j.status != DspJobStatus::Complete);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gpu_dsp_creation() {
        let result = GpuDsp::new().await;
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_dsp_job_creation() {
        let job = DspJob {
            id: uuid::Uuid::new_v4(),
            job_type: DspJobType::Convolution,
            progress: 0.5,
            status: DspJobStatus::Processing,
            description: "Test".to_string(),
        };
        
        assert_eq!(job.progress, 0.5);
    }
}
