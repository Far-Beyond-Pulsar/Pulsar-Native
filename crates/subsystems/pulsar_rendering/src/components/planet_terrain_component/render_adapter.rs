//! Renderer projection owned by [`super::PlanetTerrainComponent`].
//!
//! `pulsar_terrain` deliberately has no Helio dependency. This module is the
//! only place where its immutable render messages become Helio protocol
//! values, keeping package identity and source-generation semantics auditable.
//! The adapter is component infrastructure, not an engine subsystem: Helio
//! remains the long-lived renderer and Pulsar terrain remains authoritative.

use std::collections::{BTreeSet, VecDeque};

use helio_pass_planetary_voxel::{
    FrameUpdateOutcome, GpuResidencyError, GpuUploadOutcome, PlanetaryRenderError,
    PlanetaryVoxelRenderPass, PlanetaryVoxelResidency,
};
use helio_planet_voxel_core::{
    AddressError, ContractError, EvictOutcome, EvictedPage, LOD0_CELL_SIZE_METERS, PAGE_EDGE_CELLS,
    PageEvict, PageKey, PageUpload, PlanetFrameUniform, PlanetId, PlanetPageKey, SourceGeneration,
    UploadOutcome, VisibilityOutcome, VisiblePage, VisiblePageSet,
};
use pulsar_terrain::{
    PlanetFramePayload, TerrainPageEvict, TerrainPageUpload, TerrainPlanetEvict,
    TerrainRenderCommand, TerrainRenderCommandDisposition, TerrainRenderCommandFeedback,
    TerrainRenderCommandId, TerrainRenderDelta, TerrainRenderFeedback, TerrainVisiblePageSet,
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
    pub feedback: TerrainRenderFeedback,
}

#[derive(Debug, Error)]
pub enum PlanetaryTerrainRenderError {
    #[error(transparent)]
    Address(#[from] AddressError),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Residency(#[from] GpuResidencyError),
    #[error(transparent)]
    Render(#[from] PlanetaryRenderError),
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
    #[error("renderer apply report does not match the submitted terrain delta")]
    ApplyReportMismatch,
    #[error("planet visible sets disagree on frame index: expected {expected}, got {actual}")]
    VisibleFrameMismatch { expected: u64, actual: u64 },
    #[error("Helio's active render graph does not contain the planetary voxel pass")]
    MissingPlanetaryPass,
}

/// Stateless translation boundary into Helio's graph-owned planetary pass.
///
/// The adapter must never allocate a second residency cache: the pass retained
/// by Helio's render graph is the cache that is actually sampled and drawn.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlanetTerrainComponentRenderAdapter;

impl PlanetTerrainComponentRenderAdapter {
    pub const fn new() -> Self {
        Self
    }

    pub fn residency<'a>(
        &self,
        renderer: &'a helio::Renderer,
    ) -> Result<&'a PlanetaryVoxelResidency, PlanetaryTerrainRenderError> {
        renderer
            .find_pass::<PlanetaryVoxelRenderPass>()
            .map(PlanetaryVoxelRenderPass::residency)
            .ok_or(PlanetaryTerrainRenderError::MissingPlanetaryPass)
    }

    fn render_pass_mut<'a>(
        &self,
        renderer: &'a mut helio::Renderer,
    ) -> Result<&'a mut PlanetaryVoxelRenderPass, PlanetaryTerrainRenderError> {
        renderer
            .find_pass_mut::<PlanetaryVoxelRenderPass>()
            .ok_or(PlanetaryTerrainRenderError::MissingPlanetaryPass)
    }

    pub fn set_planet_frame(
        &self,
        renderer: &mut helio::Renderer,
        queue: &wgpu::Queue,
        frame: PlanetFramePayload,
    ) -> Result<FrameUpdateOutcome, PlanetaryTerrainRenderError> {
        Ok(self
            .render_pass_mut(renderer)?
            .set_planet_frame(queue, translate_frame(frame)?)?)
    }

    pub fn apply_delta(
        &self,
        renderer: &mut helio::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        delta: TerrainRenderDelta,
    ) -> Result<TerrainRenderApplyReport, PlanetaryTerrainRenderError> {
        let commands = delta
            .commands
            .iter()
            .map(SubmittedTerrainCommand::from)
            .collect::<Vec<_>>();
        let mut report = self.apply_batch(renderer, device, queue, translate_delta(delta)?)?;
        report.feedback = build_feedback(&commands, &report)?;
        Ok(report)
    }

    pub fn apply_batch(
        &self,
        renderer: &mut helio::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        batch: HelioTerrainRenderBatch,
    ) -> Result<TerrainRenderApplyReport, PlanetaryTerrainRenderError> {
        apply_batch_to_render_pass(self.render_pass_mut(renderer)?, device, queue, batch)
    }

    pub fn apply_visible_sets(
        &self,
        renderer: &mut helio::Renderer,
        queue: &wgpu::Queue,
        frame_index: u64,
        sets: Vec<TerrainVisiblePageSet>,
    ) -> Result<VisibilityOutcome, PlanetaryTerrainRenderError> {
        let set = translate_visible_sets(frame_index, sets)?;
        Ok(self
            .render_pass_mut(renderer)?
            .apply_visible_set(queue, set)?)
    }

    pub fn recreate_gpu_resources(
        &self,
        renderer: &mut helio::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(), PlanetaryTerrainRenderError> {
        Ok(self
            .render_pass_mut(renderer)?
            .residency_mut()
            .recreate_gpu_resources(device, queue)?)
    }
}

fn apply_batch_to_render_pass(
    pass: &mut PlanetaryVoxelRenderPass,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    batch: HelioTerrainRenderBatch,
) -> Result<TerrainRenderApplyReport, PlanetaryTerrainRenderError> {
    let chunk_size = pass.residency().config().max_batch_pages as usize;
    let mut report = TerrainRenderApplyReport::default();
    let mut uploads = VecDeque::from(batch.uploads);
    while !uploads.is_empty() {
        let count = uploads.len().min(chunk_size);
        let chunk = uploads.drain(..count).collect();
        report
            .uploads
            .extend(pass.apply_upload_batch(device, queue, chunk)?);
    }

    let mut evictions = VecDeque::from(batch.evictions);
    while !evictions.is_empty() {
        let count = evictions.len().min(chunk_size);
        let chunk = evictions.drain(..count).collect();
        report
            .evictions
            .extend(pass.apply_evict_batch(device, queue, chunk)?);
    }

    for planet in batch.retired_planets {
        let retirement = match pass.residency_mut().remove_planet_frame(planet) {
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

#[cfg(test)]
fn apply_batch_to_residency(
    residency: &mut PlanetaryVoxelResidency,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    batch: HelioTerrainRenderBatch,
) -> Result<TerrainRenderApplyReport, PlanetaryTerrainRenderError> {
    let chunk_size = residency.config().max_batch_pages as usize;
    let mut report = TerrainRenderApplyReport::default();
    let mut uploads = VecDeque::from(batch.uploads);
    while !uploads.is_empty() {
        let count = uploads.len().min(chunk_size);
        let chunk = uploads.drain(..count).collect();
        report
            .uploads
            .extend(residency.apply_upload_batch(device, queue, chunk)?);
    }

    let mut evictions = VecDeque::from(batch.evictions);
    while !evictions.is_empty() {
        let count = evictions.len().min(chunk_size);
        let chunk = evictions.drain(..count).collect();
        report
            .evictions
            .extend(residency.apply_evict_batch(device, queue, chunk)?);
    }

    for planet in batch.retired_planets {
        let retirement = match residency.remove_planet_frame(planet) {
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

/// Lightweight command identity retained while the payload is moved into
/// Helio. Upload pages are large; feedback construction must never clone their
/// dense cell buffers on the render path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubmittedTerrainCommand {
    Upload(TerrainRenderCommandId),
    EvictPage(TerrainRenderCommandId),
    EvictPlanet {
        id: TerrainRenderCommandId,
        planet_id: PlanetId,
        page_count: usize,
    },
}

impl From<&TerrainRenderCommand> for SubmittedTerrainCommand {
    fn from(command: &TerrainRenderCommand) -> Self {
        let id = command.id();
        match command {
            TerrainRenderCommand::Upload(_) => Self::Upload(id),
            TerrainRenderCommand::EvictPage(_) => Self::EvictPage(id),
            TerrainRenderCommand::EvictPlanet(eviction) => Self::EvictPlanet {
                id,
                planet_id: translate_planet_id(eviction.planet_id),
                page_count: eviction.pages.len(),
            },
        }
    }
}

impl SubmittedTerrainCommand {
    const fn feedback(
        self,
        disposition: TerrainRenderCommandDisposition,
    ) -> TerrainRenderCommandFeedback {
        let command = match self {
            Self::Upload(id) | Self::EvictPage(id) | Self::EvictPlanet { id, .. } => id,
        };
        TerrainRenderCommandFeedback {
            command,
            disposition,
        }
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

fn build_feedback(
    commands: &[SubmittedTerrainCommand],
    report: &TerrainRenderApplyReport,
) -> Result<TerrainRenderFeedback, PlanetaryTerrainRenderError> {
    let mut upload_index = 0;
    let mut eviction_index = 0;
    let mut feedback = TerrainRenderFeedback::default();

    for command in commands {
        let disposition = match command {
            SubmittedTerrainCommand::Upload(_) => {
                let outcome = report
                    .uploads
                    .get(upload_index)
                    .ok_or(PlanetaryTerrainRenderError::ApplyReportMismatch)?;
                upload_index += 1;
                if let GpuUploadOutcome::Residency(UploadOutcome::Inserted { evicted, .. }) =
                    outcome
                {
                    feedback
                        .cache_evictions
                        .extend(evicted.iter().map(translate_cache_eviction));
                }
                match outcome {
                    GpuUploadOutcome::Residency(
                        UploadOutcome::Inserted { .. }
                        | UploadOutcome::Replaced { .. }
                        | UploadOutcome::Duplicate { .. },
                    ) => TerrainRenderCommandDisposition::Applied,
                    GpuUploadOutcome::Residency(UploadOutcome::Backpressure(_))
                    | GpuUploadOutcome::PageTableBackpressure => {
                        TerrainRenderCommandDisposition::Deferred
                    }
                    GpuUploadOutcome::Residency(
                        UploadOutcome::Stale { .. } | UploadOutcome::GenerationConflict { .. },
                    ) => TerrainRenderCommandDisposition::Rejected,
                }
            }
            SubmittedTerrainCommand::EvictPage(_) => {
                let outcome = report
                    .evictions
                    .get(eviction_index)
                    .ok_or(PlanetaryTerrainRenderError::ApplyReportMismatch)?;
                eviction_index += 1;
                eviction_disposition(outcome)
            }
            SubmittedTerrainCommand::EvictPlanet {
                planet_id,
                page_count,
                ..
            } => {
                let end = eviction_index
                    .checked_add(*page_count)
                    .ok_or(PlanetaryTerrainRenderError::ApplyReportMismatch)?;
                let outcomes = report
                    .evictions
                    .get(eviction_index..end)
                    .ok_or(PlanetaryTerrainRenderError::ApplyReportMismatch)?;
                eviction_index = end;
                let pages_applied = outcomes.iter().all(|outcome| {
                    eviction_disposition(outcome) == TerrainRenderCommandDisposition::Applied
                });
                let frame_applied = report.frame_retirements.iter().any(|retirement| {
                    matches!(
                        retirement,
                        PlanetFrameRetirement::Removed(planet)
                            | PlanetFrameRetirement::AlreadyAbsent(planet)
                            if planet == planet_id
                    )
                });
                if pages_applied && frame_applied {
                    TerrainRenderCommandDisposition::Applied
                } else {
                    TerrainRenderCommandDisposition::Deferred
                }
            }
        };
        feedback.commands.push(command.feedback(disposition));
    }

    if upload_index != report.uploads.len() || eviction_index != report.evictions.len() {
        return Err(PlanetaryTerrainRenderError::ApplyReportMismatch);
    }
    Ok(feedback)
}

const fn eviction_disposition(outcome: &EvictOutcome) -> TerrainRenderCommandDisposition {
    match outcome {
        EvictOutcome::Recorded { .. } | EvictOutcome::Stale { .. } => {
            TerrainRenderCommandDisposition::Applied
        }
        EvictOutcome::Backpressure(_) => TerrainRenderCommandDisposition::Deferred,
    }
}

fn translate_cache_eviction(eviction: &EvictedPage) -> TerrainPageEvict {
    TerrainPageEvict {
        planet_id: pulsar_terrain::PlanetId(eviction.key.planet.0),
        page_key: pulsar_terrain::PageKey::new(eviction.key.page.lod, eviction.key.page.page_xyz),
        planet_generation: eviction.generation.planet,
        page_generation: eviction.generation.page,
    }
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

/// Merge every planet's complete committed frontier into the one global set
/// consumed by Helio. Applying sets one at a time would replace the previous
/// planet's visibility and make solar-system-scale multi-planet rendering
/// impossible.
pub fn translate_visible_sets(
    expected_frame_index: u64,
    sets: Vec<TerrainVisiblePageSet>,
) -> Result<VisiblePageSet, PlanetaryTerrainRenderError> {
    let mut pages = Vec::new();
    for set in sets {
        if set.frame_index != expected_frame_index {
            return Err(PlanetaryTerrainRenderError::VisibleFrameMismatch {
                expected: expected_frame_index,
                actual: set.frame_index,
            });
        }
        pages.extend(translate_visible_set(set)?.pages);
    }
    pages.sort_unstable_by_key(|page| page.key);
    let translated = VisiblePageSet {
        frame_index: expected_frame_index,
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
    use helio_pass_planetary_voxel::{PlanetarySurfaceRequest, PlanetaryVoxelGpuConfig};
    use helio_planet_voxel_core::{
        PAGE_CELL_COUNT, PAGE_EDGE as HELIO_PAGE_EDGE, PlanetPageKey, SourceGeneration,
        TRANSITION_FACE_MASK, UploadOutcome,
    };
    use pulsar_terrain::{
        CELL_COUNT, CellWord as TerrainCellWord, LOD0_CELL_SIZE_METERS as TERRAIN_CELL_SIZE_METERS,
        PAGE_EDGE, PageKey as TerrainPageKey, PlanetFrame, PlanetId as TerrainId, PlanetPosition,
        TERRAIN_TRANSITION_FACE_MASK, TerrainRenderDeltaCounters, TerrainVisiblePage,
        terrain_surface_required_pages,
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

    fn test_gpu_config() -> PlanetaryVoxelGpuConfig {
        PlanetaryVoxelGpuConfig {
            max_resident_pages: 2,
            table_capacity: 8,
            max_probe: 8,
            max_batch_pages: 1,
            max_eviction_watermarks: 4,
            ..PlanetaryVoxelGpuConfig::default()
        }
    }

    fn apply_delta_to_test_residency(
        residency: &mut PlanetaryVoxelResidency,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        delta: TerrainRenderDelta,
    ) -> Result<TerrainRenderApplyReport, PlanetaryTerrainRenderError> {
        let commands = delta
            .commands
            .iter()
            .map(SubmittedTerrainCommand::from)
            .collect::<Vec<_>>();
        let mut report =
            apply_batch_to_residency(residency, device, queue, translate_delta(delta)?)?;
        report.feedback = build_feedback(&commands, &report)?;
        Ok(report)
    }

    #[test]
    fn pulsar_and_helio_share_the_same_voxel_protocol_constants() {
        assert_eq!(TERRAIN_CELL_SIZE_METERS, LOD0_CELL_SIZE_METERS);
        assert_eq!(PAGE_EDGE, HELIO_PAGE_EDGE);
        assert_eq!(CELL_COUNT, PAGE_CELL_COUNT);
        assert_eq!(TERRAIN_TRANSITION_FACE_MASK, TRANSITION_FACE_MASK);
        assert_eq!(
            core::mem::size_of::<PlanetTerrainComponentRenderAdapter>(),
            0,
            "the component adapter must not own a duplicate GPU cache"
        );
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
    fn page_table_backpressure_is_reported_as_deferred_feedback() {
        let command = TerrainRenderCommand::Upload(terrain_upload(
            4,
            9,
            TerrainPageKey::new(1, [2, -3, 4]),
            TerrainCellWord::AIR,
        ));
        let id = command.id();
        let report = TerrainRenderApplyReport {
            uploads: vec![GpuUploadOutcome::PageTableBackpressure],
            ..TerrainRenderApplyReport::default()
        };
        let feedback = build_feedback(&[SubmittedTerrainCommand::from(&command)], &report).unwrap();
        assert_eq!(
            feedback.commands,
            vec![TerrainRenderCommandFeedback {
                command: id,
                disposition: TerrainRenderCommandDisposition::Deferred,
            }]
        );
        assert!(feedback.cache_evictions.is_empty());
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
    fn pulsar_sampling_dependencies_match_helio_exactly() {
        for (page, transition_mask) in [
            (TerrainPageKey::new(1, [0, 0, 0]), 0),
            (TerrainPageKey::new(3, [-17, 4, -9]), 0b00_111111),
            (
                TerrainPageKey::new(7, [i32::MAX as i64, -3, 11]),
                0b00_010101,
            ),
        ] {
            let pulsar = terrain_surface_required_pages(page, transition_mask).unwrap();
            let helio = PlanetarySurfaceRequest {
                key: PlanetPageKey::new(
                    PlanetId([9; 16]),
                    helio_planet_voxel_core::PageKey::new(page.lod, page.page_xyz),
                ),
                generation: SourceGeneration::new(1, 2),
                transition_mask,
                dirty_microbricks: u64::MAX,
            }
            .required_pages()
            .unwrap()
            .into_iter()
            .map(|key| TerrainPageKey::new(key.page.lod, key.page.page_xyz))
            .collect::<BTreeSet<_>>();
            assert_eq!(pulsar, helio);
        }
    }

    #[test]
    fn visible_frontiers_from_multiple_planets_form_one_global_publication() {
        let make_set = |planet: u8, x: i64, frame_index| TerrainVisiblePageSet {
            planet_id: TerrainId([planet; 16]),
            frame_index,
            pages: vec![TerrainVisiblePage {
                page_key: TerrainPageKey::new(3, [x, 0, 0]),
                planet_generation: 1,
                page_generation: 2,
                transition_mask: 0,
            }],
        };

        let combined =
            translate_visible_sets(77, vec![make_set(1, -1, 77), make_set(2, 1, 77)]).unwrap();
        assert_eq!(combined.frame_index, 77);
        assert_eq!(combined.pages.len(), 2);
        assert_eq!(combined.pages[0].key.planet, PlanetId([1; 16]));
        assert_eq!(combined.pages[1].key.planet, PlanetId([2; 16]));

        assert!(matches!(
            translate_visible_sets(77, vec![make_set(1, 0, 77), make_set(2, 0, 78)]),
            Err(PlanetaryTerrainRenderError::VisibleFrameMismatch {
                expected: 77,
                actual: 78
            })
        ));

        let empty = translate_visible_sets(79, Vec::new()).unwrap();
        assert_eq!(empty.frame_index, 79);
        assert!(empty.pages.is_empty());
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
            let mut residency =
                PlanetaryVoxelResidency::new(&device, &queue, test_gpu_config()).unwrap();
            let planet = TerrainId([7; 16]);
            residency
                .set_planet_frame(
                    &queue,
                    translate_frame(
                        PlanetFrame::new(planet, PlanetPosition::from_lod0_cell([0; 3]), 1)
                            .renderer_payload(),
                    )
                    .unwrap(),
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
            let first_id = first.commands[0].id();
            let first_report =
                apply_delta_to_test_residency(&mut residency, &device, &queue, first).unwrap();
            assert!(matches!(
                first_report.uploads.as_slice(),
                [GpuUploadOutcome::Residency(UploadOutcome::Inserted { .. })]
            ));
            assert_eq!(
                first_report.feedback.commands,
                vec![TerrainRenderCommandFeedback {
                    command: first_id,
                    disposition: TerrainRenderCommandDisposition::Applied,
                }]
            );

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
                apply_delta_to_test_residency(&mut residency, &device, &queue, replacement,)
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
            let stale_id = stale.commands[0].id();
            let stale_report =
                apply_delta_to_test_residency(&mut residency, &device, &queue, stale).unwrap();
            assert!(matches!(
                stale_report.uploads.as_slice(),
                [GpuUploadOutcome::Residency(UploadOutcome::Stale {
                    newest_generation
                })] if *newest_generation == SourceGeneration::new(2, 0)
            ));
            assert_eq!(
                stale_report.feedback.commands,
                vec![TerrainRenderCommandFeedback {
                    command: stale_id,
                    disposition: TerrainRenderCommandDisposition::Rejected,
                }]
            );
            let resident = residency
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
            let retirement_id = retirement.commands[0].id();
            let retired =
                apply_delta_to_test_residency(&mut residency, &device, &queue, retirement).unwrap();
            assert!(matches!(
                retired.evictions.as_slice(),
                [EvictOutcome::Recorded { removed: Some(_) }]
            ));
            assert_eq!(
                retired.frame_retirements,
                vec![PlanetFrameRetirement::Removed(PlanetId([7; 16]))]
            );
            assert_eq!(
                retired.feedback.commands,
                vec![TerrainRenderCommandFeedback {
                    command: retirement_id,
                    disposition: TerrainRenderCommandDisposition::Applied,
                }]
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
                apply_delta_to_test_residency(
                    &mut residency,
                    &device,
                    &queue,
                    after_retirement,
                ),
                Err(PlanetaryTerrainRenderError::Residency(
                    GpuResidencyError::MissingPlanetFrame(PlanetId(id))
                )) if id == [7; 16]
            ));
        });
    }
}
