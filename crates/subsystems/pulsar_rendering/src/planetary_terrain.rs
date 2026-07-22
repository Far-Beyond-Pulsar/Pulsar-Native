//! Explicit boundary between Pulsar's authoritative terrain runtime and
//! Helio's disposable planetary GPU cache.
//!
//! `pulsar_terrain` deliberately has no Helio dependency. This module is the
//! only place where its immutable render messages become Helio protocol
//! values, keeping package identity and source-generation semantics auditable.

use std::collections::{BTreeSet, VecDeque};

use helio_pass_planetary_voxel::{
    FrameUpdateOutcome, GpuResidencyError, GpuUploadOutcome, PlanetaryVoxelGpuConfig,
    PlanetaryVoxelResidency,
};
use helio_planet_voxel_core::{
    AddressError, ContractError, EvictOutcome, PageEvict, PageKey, PageUpload, PlanetFrameUniform,
    PlanetId, PlanetPageKey, SourceGeneration, VisibilityOutcome, VisiblePage, VisiblePageSet,
    LOD0_CELL_SIZE_METERS, PAGE_EDGE_CELLS,
};
use pulsar_terrain::{
    PlanetFramePayload, TerrainPageEvict, TerrainPageUpload, TerrainPlanetEvict,
    TerrainRenderCommand, TerrainRenderDelta, TerrainVisiblePageSet,
};
use thiserror::Error;

/// Fully translated, renderer-owned batch. Translation validates every value
/// before the GPU residency cache is mutated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HelioTerrainRenderBatch {
    pub uploads: Vec<PageUpload>,
    pub evictions: Vec<PageEvict>,
    pub retired_planets: Vec<PlanetId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanetFrameRetirement {
    Removed(PlanetId),
    RetainedInUse(PlanetId),
    AlreadyAbsent(PlanetId),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainRenderApplyReport {
    pub uploads: Vec<GpuUploadOutcome>,
    pub evictions: Vec<EvictOutcome>,
    pub frame_retirements: Vec<PlanetFrameRetirement>,
}

#[derive(Debug, Error)]
pub enum PlanetaryTerrainRenderError {
    #[error(transparent)]
    Address(#[from] AddressError),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Residency(#[from] GpuResidencyError),
    #[error("planet frame contains non-finite camera-relative coordinates")]
    NonFiniteFrame,
    #[error("planet frame LOD0 cell size {actual} does not match Helio's {expected}")]
    CellSizeMismatch { actual: f32, expected: f32 },
    #[error("planet frame page edge {actual} does not match Helio's {expected}")]
    PageEdgeMismatch { actual: u32, expected: u32 },
    #[error("planet eviction for {planet:?} contains a page owned by {page_planet:?}")]
    PlanetEvictionMismatch {
        planet: pulsar_terrain::PlanetId,
        page_planet: pulsar_terrain::PlanetId,
    },
    #[error(
        "planet eviction retires generation {retired}, but a listed page belongs to newer generation {page}"
    )]
    PlanetEvictionGeneration { retired: u64, page: u64 },
}

/// Owns Helio's bounded planetary residency while leaving canonical terrain,
/// scheduling, persistence, and event ownership in `pulsar_terrain`.
pub struct PlanetaryTerrainRenderAdapter {
    residency: PlanetaryVoxelResidency,
}

impl PlanetaryTerrainRenderAdapter {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: PlanetaryVoxelGpuConfig,
    ) -> Result<Self, PlanetaryTerrainRenderError> {
        Ok(Self {
            residency: PlanetaryVoxelResidency::new(device, queue, config)?,
        })
    }

    pub const fn residency(&self) -> &PlanetaryVoxelResidency {
        &self.residency
    }

    pub fn residency_mut(&mut self) -> &mut PlanetaryVoxelResidency {
        &mut self.residency
    }

    pub fn set_planet_frame(
        &mut self,
        queue: &wgpu::Queue,
        frame: PlanetFramePayload,
    ) -> Result<FrameUpdateOutcome, PlanetaryTerrainRenderError> {
        Ok(self
            .residency
            .set_planet_frame(queue, translate_frame(frame)?)?)
    }

    pub fn apply_delta(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        delta: TerrainRenderDelta,
    ) -> Result<TerrainRenderApplyReport, PlanetaryTerrainRenderError> {
        self.apply_batch(device, queue, translate_delta(delta)?)
    }

    pub fn apply_batch(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        batch: HelioTerrainRenderBatch,
    ) -> Result<TerrainRenderApplyReport, PlanetaryTerrainRenderError> {
        let chunk_size = self.residency.config().max_batch_pages as usize;
        let mut report = TerrainRenderApplyReport::default();
        let mut uploads = VecDeque::from(batch.uploads);
        while !uploads.is_empty() {
            let count = uploads.len().min(chunk_size);
            let chunk = uploads.drain(..count).collect();
            report
                .uploads
                .extend(self.residency.apply_upload_batch(device, queue, chunk)?);
        }

        let mut evictions = VecDeque::from(batch.evictions);
        while !evictions.is_empty() {
            let count = evictions.len().min(chunk_size);
            let chunk = evictions.drain(..count).collect();
            report
                .evictions
                .extend(self.residency.apply_evict_batch(device, queue, chunk)?);
        }

        for planet in batch.retired_planets {
            let retirement = match self.residency.remove_planet_frame(planet) {
                Ok(true) => PlanetFrameRetirement::Removed(planet),
                Ok(false) => PlanetFrameRetirement::AlreadyAbsent(planet),
                Err(GpuResidencyError::PlanetFrameInUse(_)) => {
                    PlanetFrameRetirement::RetainedInUse(planet)
                }
                Err(error) => return Err(error.into()),
            };
            report.frame_retirements.push(retirement);
        }
        Ok(report)
    }

    pub fn apply_visible_set(
        &mut self,
        queue: &wgpu::Queue,
        set: TerrainVisiblePageSet,
    ) -> Result<VisibilityOutcome, PlanetaryTerrainRenderError> {
        Ok(self
            .residency
            .apply_visible_set(queue, translate_visible_set(set)?)?)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.residency.resize(width, height);
    }

    pub fn recreate_gpu_resources(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(), PlanetaryTerrainRenderError> {
        Ok(self.residency.recreate_gpu_resources(device, queue)?)
    }
}

pub fn translate_delta(
    delta: TerrainRenderDelta,
) -> Result<HelioTerrainRenderBatch, PlanetaryTerrainRenderError> {
    let mut batch = HelioTerrainRenderBatch::default();
    let mut retired_planets = BTreeSet::new();
    for command in delta.commands {
        match command {
            TerrainRenderCommand::Upload(upload) => {
                batch.uploads.push(translate_upload(upload)?);
            }
            TerrainRenderCommand::EvictPage(eviction) => {
                batch.evictions.push(translate_evict(eviction)?);
            }
            TerrainRenderCommand::EvictPlanet(eviction) => {
                translate_planet_evict(eviction, &mut batch.evictions, &mut retired_planets)?;
            }
        }
    }
    batch.retired_planets = retired_planets.into_iter().collect();
    Ok(batch)
}

pub fn translate_visible_set(
    set: TerrainVisiblePageSet,
) -> Result<VisiblePageSet, PlanetaryTerrainRenderError> {
    let planet = translate_planet_id(set.planet_id);
    let pages = set
        .pages
        .into_iter()
        .map(|page| {
            Ok(VisiblePage {
                key: PlanetPageKey::new(planet, translate_page_key(page.page_key)?),
                generation: SourceGeneration::new(page.planet_generation, page.page_generation),
                transition_mask: page.transition_mask,
            })
        })
        .collect::<Result<Vec<_>, PlanetaryTerrainRenderError>>()?;
    let translated = VisiblePageSet {
        frame_index: set.frame_index,
        pages,
    };
    translated.validate(translated.pages.len())?;
    Ok(translated)
}

pub fn translate_frame(
    frame: PlanetFramePayload,
) -> Result<PlanetFrameUniform, PlanetaryTerrainRenderError> {
    if frame
        .camera_relative_m()
        .into_iter()
        .any(|value| !value.is_finite())
    {
        return Err(PlanetaryTerrainRenderError::NonFiniteFrame);
    }
    let expected_cell_size = LOD0_CELL_SIZE_METERS as f32;
    if frame.lod0_cell_size_m() != expected_cell_size {
        return Err(PlanetaryTerrainRenderError::CellSizeMismatch {
            actual: frame.lod0_cell_size_m(),
            expected: expected_cell_size,
        });
    }
    let expected_page_edge = PAGE_EDGE_CELLS as u32;
    if frame.page_edge_cells() != expected_page_edge {
        return Err(PlanetaryTerrainRenderError::PageEdgeMismatch {
            actual: frame.page_edge_cells(),
            expected: expected_page_edge,
        });
    }
    let origin = frame.origin_words();
    Ok(PlanetFrameUniform {
        planet_id: frame.planet_id_words(),
        origin_x: origin[0],
        origin_y: origin[1],
        origin_z: origin[2],
        frame_index: frame.frame_index_words(),
        camera_relative_m: frame.camera_relative_m(),
        lod0_cell_size_m: frame.lod0_cell_size_m(),
        page_edge_cells: frame.page_edge_cells(),
        _pad: [0; 3],
    })
}

fn translate_upload(upload: TerrainPageUpload) -> Result<PageUpload, PlanetaryTerrainRenderError> {
    let key = PlanetPageKey::new(
        translate_planet_id(upload.planet_id),
        translate_page_key(upload.page_key)?,
    );
    let cells = upload
        .cells
        .into_vec()
        .into_iter()
        .map(|cell| helio_planet_voxel_core::CellWord(cell.0))
        .collect();
    Ok(PageUpload::new(
        key,
        SourceGeneration::new(upload.planet_generation, upload.page_generation),
        cells,
    )?)
}

fn translate_evict(eviction: TerrainPageEvict) -> Result<PageEvict, PlanetaryTerrainRenderError> {
    let translated = PageEvict {
        key: PlanetPageKey::new(
            translate_planet_id(eviction.planet_id),
            translate_page_key(eviction.page_key)?,
        ),
        generation: SourceGeneration::new(eviction.planet_generation, eviction.page_generation),
    };
    translated.validate()?;
    Ok(translated)
}

fn translate_planet_evict(
    eviction: TerrainPlanetEvict,
    output: &mut Vec<PageEvict>,
    retired_planets: &mut BTreeSet<PlanetId>,
) -> Result<(), PlanetaryTerrainRenderError> {
    for page in eviction.pages {
        if page.planet_id != eviction.planet_id {
            return Err(PlanetaryTerrainRenderError::PlanetEvictionMismatch {
                planet: eviction.planet_id,
                page_planet: page.planet_id,
            });
        }
        if page.planet_generation > eviction.retired_planet_generation {
            return Err(PlanetaryTerrainRenderError::PlanetEvictionGeneration {
                retired: eviction.retired_planet_generation,
                page: page.planet_generation,
            });
        }
        output.push(translate_evict(page)?);
    }
    retired_planets.insert(translate_planet_id(eviction.planet_id));
    Ok(())
}

fn translate_planet_id(planet: pulsar_terrain::PlanetId) -> PlanetId {
    PlanetId(planet.0)
}

fn translate_page_key(page: pulsar_terrain::PageKey) -> Result<PageKey, AddressError> {
    let translated = PageKey::new(page.lod, page.page_xyz);
    translated.validate()?;
    Ok(translated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_planet_voxel_core::{
        UploadOutcome, PAGE_CELL_COUNT, PAGE_EDGE as HELIO_PAGE_EDGE, TRANSITION_FACE_MASK,
    };
    use pulsar_terrain::{
        CellWord as TerrainCellWord, PageKey as TerrainPageKey, PlanetFrame, PlanetId as TerrainId,
        PlanetPosition, TerrainRenderDeltaCounters, TerrainVisiblePage, CELL_COUNT,
        LOD0_CELL_SIZE_METERS as TERRAIN_CELL_SIZE_METERS, PAGE_EDGE, TERRAIN_TRANSITION_FACE_MASK,
    };

    fn terrain_upload(
        planet_generation: u64,
        page_generation: u64,
        page: TerrainPageKey,
        cell: TerrainCellWord,
    ) -> TerrainPageUpload {
        TerrainPageUpload {
            planet_id: TerrainId([7; 16]),
            page_key: page,
            planet_generation,
            page_generation,
            cells: vec![cell; PAGE_CELL_COUNT].into_boxed_slice(),
        }
    }

    #[test]
    fn pulsar_and_helio_share_the_same_voxel_protocol_constants() {
        assert_eq!(TERRAIN_CELL_SIZE_METERS, LOD0_CELL_SIZE_METERS);
        assert_eq!(PAGE_EDGE, HELIO_PAGE_EDGE);
        assert_eq!(CELL_COUNT, PAGE_CELL_COUNT);
        assert_eq!(TERRAIN_TRANSITION_FACE_MASK, TRANSITION_FACE_MASK);
    }

    #[test]
    fn translation_preserves_signed_addresses_cells_and_source_generations() {
        let delta = TerrainRenderDelta {
            commands: vec![TerrainRenderCommand::Upload(terrain_upload(
                9,
                u64::MAX,
                TerrainPageKey::new(4, [-17, 3, -1]),
                TerrainCellWord::new(-123, 19, 5),
            ))],
            counters: TerrainRenderDeltaCounters::default(),
        };
        let batch = translate_delta(delta).unwrap();
        assert_eq!(batch.uploads.len(), 1);
        let upload = &batch.uploads[0];
        assert_eq!(upload.key.planet, PlanetId([7; 16]));
        assert_eq!(upload.key.page, PageKey::new(4, [-17, 3, -1]));
        assert_eq!(upload.generation, SourceGeneration::new(9, u64::MAX));
        assert_eq!(upload.cells[0].0, TerrainCellWord::new(-123, 19, 5).0);
    }

    #[test]
    fn visible_translation_preserves_all_transition_bits_and_rejects_invalid_lod() {
        let set = TerrainVisiblePageSet {
            planet_id: TerrainId([3; 16]),
            frame_index: 44,
            pages: vec![TerrainVisiblePage {
                page_key: TerrainPageKey::new(2, [-3, 4, -5]),
                planet_generation: 8,
                page_generation: 13,
                transition_mask: 0b00_111111,
            }],
        };
        let translated = translate_visible_set(set).unwrap();
        assert_eq!(translated.frame_index, 44);
        assert_eq!(translated.pages[0].transition_mask, 0b00_111111);
        assert_eq!(translated.pages[0].generation, SourceGeneration::new(8, 13));

        let invalid = TerrainVisiblePageSet {
            planet_id: TerrainId([3; 16]),
            frame_index: 45,
            pages: vec![TerrainVisiblePage {
                page_key: TerrainPageKey::new(u8::MAX, [0; 3]),
                planet_generation: 8,
                page_generation: 14,
                transition_mask: 0,
            }],
        };
        assert!(matches!(
            translate_visible_set(invalid),
            Err(PlanetaryTerrainRenderError::Address(
                AddressError::UnsupportedLod(u8::MAX)
            ))
        ));
    }

    #[test]
    fn frame_translation_is_field_exact_at_signed_planet_scale() {
        let terrain = PlanetFrame::new(
            TerrainId([0x91; 16]),
            PlanetPosition::new([-63_710_017, 63_710_033, -1], [0.025, 0.075, 0.099]).unwrap(),
            u64::MAX - 2,
        );
        let payload = terrain.renderer_payload();
        let frame = translate_frame(payload).unwrap();
        assert_eq!(frame.planet_id(), PlanetId([0x91; 16]));
        assert_eq!(frame.frame_origin_lod0_cell(), terrain.origin_lod0_cell());
        assert_eq!(frame.frame_number(), u64::MAX - 2);
        assert_eq!(frame.camera_relative_m, payload.camera_relative_m());
    }

    #[test]
    fn planet_eviction_validates_ownership_before_translation() {
        let delta = TerrainRenderDelta {
            commands: vec![TerrainRenderCommand::EvictPlanet(TerrainPlanetEvict {
                planet_id: TerrainId([1; 16]),
                retired_planet_generation: 4,
                pages: vec![TerrainPageEvict {
                    planet_id: TerrainId([2; 16]),
                    page_key: TerrainPageKey::new(0, [0; 3]),
                    planet_generation: 4,
                    page_generation: 1,
                }],
            })],
            counters: TerrainRenderDeltaCounters::default(),
        };
        assert!(matches!(
            translate_delta(delta),
            Err(PlanetaryTerrainRenderError::PlanetEvictionMismatch { .. })
        ));
    }

    #[test]
    fn headless_adapter_preserves_generation_order_and_retires_planet() {
        pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let mut adapter = None;
            for force_fallback_adapter in [false, true] {
                if let Ok(found) = instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        compatible_surface: None,
                        force_fallback_adapter,
                        apply_limit_buckets: false,
                    })
                    .await
                {
                    adapter = Some(found);
                    break;
                }
            }
            let Some(gpu) = adapter else {
                eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER");
                return;
            };
            let (device, queue) = gpu
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("Pulsar planetary adapter test device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: gpu.limits(),
                    ..Default::default()
                })
                .await
                .unwrap();
            let mut renderer = PlanetaryTerrainRenderAdapter::new(
                &device,
                &queue,
                PlanetaryVoxelGpuConfig::new(2, 8, 8, 1, 4, 2).unwrap(),
            )
            .unwrap();
            let planet = TerrainId([7; 16]);
            renderer
                .set_planet_frame(
                    &queue,
                    PlanetFrame::new(planet, PlanetPosition::from_lod0_cell([0; 3]), 1)
                        .renderer_payload(),
                )
                .unwrap();
            let page = TerrainPageKey::new(0, [-1, 0, 1]);
            let first = TerrainRenderDelta {
                commands: vec![TerrainRenderCommand::Upload(terrain_upload(
                    1,
                    u64::MAX,
                    page,
                    TerrainCellWord::AIR,
                ))],
                counters: TerrainRenderDeltaCounters::default(),
            };
            assert!(matches!(
                renderer
                    .apply_delta(&device, &queue, first)
                    .unwrap()
                    .uploads
                    .as_slice(),
                [GpuUploadOutcome::Residency(UploadOutcome::Inserted { .. })]
            ));

            let replacement_cell = TerrainCellWord::new(-777, 22, 4);
            let replacement = TerrainRenderDelta {
                commands: vec![TerrainRenderCommand::Upload(terrain_upload(
                    2,
                    0,
                    page,
                    replacement_cell,
                ))],
                counters: TerrainRenderDeltaCounters::default(),
            };
            assert!(matches!(
                renderer
                    .apply_delta(&device, &queue, replacement)
                    .unwrap()
                    .uploads
                    .as_slice(),
                [GpuUploadOutcome::Residency(UploadOutcome::Replaced { .. })]
            ));

            let stale = TerrainRenderDelta {
                commands: vec![TerrainRenderCommand::Upload(terrain_upload(
                    1,
                    u64::MAX,
                    page,
                    TerrainCellWord::AIR,
                ))],
                counters: TerrainRenderDeltaCounters::default(),
            };
            assert!(matches!(
                renderer.apply_delta(&device, &queue, stale).unwrap().uploads.as_slice(),
                [GpuUploadOutcome::Residency(UploadOutcome::Stale {
                    newest_generation
                })] if *newest_generation == SourceGeneration::new(2, 0)
            ));
            let resident = renderer
                .residency()
                .cache()
                .resident(PlanetPageKey::new(
                    PlanetId([7; 16]),
                    PageKey::new(0, [-1, 0, 1]),
                ))
                .unwrap();
            assert_eq!(resident.generation, SourceGeneration::new(2, 0));
            assert_eq!(resident.cells[0].0, replacement_cell.0);

            let retirement = TerrainRenderDelta {
                commands: vec![TerrainRenderCommand::EvictPlanet(TerrainPlanetEvict {
                    planet_id: planet,
                    retired_planet_generation: 2,
                    pages: vec![TerrainPageEvict {
                        planet_id: planet,
                        page_key: page,
                        planet_generation: 2,
                        page_generation: 0,
                    }],
                })],
                counters: TerrainRenderDeltaCounters::default(),
            };
            let retired = renderer.apply_delta(&device, &queue, retirement).unwrap();
            assert!(matches!(
                retired.evictions.as_slice(),
                [EvictOutcome::Recorded { removed: Some(_) }]
            ));
            assert_eq!(
                retired.frame_retirements,
                vec![PlanetFrameRetirement::Removed(PlanetId([7; 16]))]
            );

            let after_retirement = TerrainRenderDelta {
                commands: vec![TerrainRenderCommand::Upload(terrain_upload(
                    3,
                    0,
                    page,
                    replacement_cell,
                ))],
                counters: TerrainRenderDeltaCounters::default(),
            };
            assert!(matches!(
                renderer.apply_delta(&device, &queue, after_retirement),
                Err(PlanetaryTerrainRenderError::Residency(
                    GpuResidencyError::MissingPlanetFrame(PlanetId(id))
                )) if id == [7; 16]
            ));
        });
    }
}
