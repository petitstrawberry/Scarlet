//! Queries span submitted passes; each pass contributes a real GPU result.
use super::*;
#[derive(Default)]
pub(super) struct State {
    active: [Option<Scope>; 2],
}
struct Scope {
    mask: u32,
    pending: Vec<(raw::Buffer, usize)>,
    assembly: [u64; 2],
}
pub(super) struct PassQueries {
    pub(super) occlusion: Option<raw::QuerySet>,
    pub(super) statistics: Option<raw::QuerySet>,
}
impl Context {
    /// Query flags: non-precise occlusion (1), optional pipeline statistics (4).
    /// WGPU does not guarantee precise sample counts, including on Metal.
    pub fn query_support(&self) -> u32 {
        1 | if self
            .raw_device()
            .features()
            .contains(raw::Features::PIPELINE_STATISTICS_QUERY)
        {
            4
        } else {
            0
        }
    }
    /// Begin a query scope. Kinds are 0=occlusion and 1=pipeline statistics.
    pub fn begin_query(&self, kind: u32, mask: u32) -> Result<()> {
        if kind > 1
            || (kind == 1 && (self.query_support() & 4 == 0 || mask == 0 || mask & !0x7ff != 0))
        {
            return Err(Error::InvalidState);
        }
        let mut s = self.queries.lock().map_err(|_| Error::InvalidState)?;
        let slot = &mut s.active[kind as usize];
        if slot.is_some() {
            return Err(Error::InvalidState);
        }
        *slot = Some(Scope {
            mask,
            pending: Vec::new(),
            assembly: [0; 2],
        });
        Ok(())
    }
    /// Retire results from every contributing GPU pass and sum in Vulkan bit order.
    pub fn end_query(&self, kind: u32) -> Result<Vec<u64>> {
        if kind > 1 {
            return Err(Error::InvalidState);
        }
        let scope = self.queries.lock().map_err(|_| Error::InvalidState)?.active[kind as usize]
            .take()
            .ok_or(Error::InvalidState)?;
        let mut totals = [0u64; 11];
        totals[..2].copy_from_slice(&scope.assembly);
        for (buffer, count) in scope.pending {
            let slice = buffer.slice(..);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            slice.map_async(raw::MapMode::Read, move |v| {
                let _ = tx.send(v);
            });
            let _ = self.raw_device().poll(raw::Maintain::Wait);
            rx.recv()
                .map_err(|_| Error::DeviceLost)?
                .map_err(|_| Error::DeviceLost)?;
            let bytes = slice.get_mapped_range();
            let indices: &[usize] = if kind == 0 { &[0] } else { &[2, 5, 6, 7, 10] };
            for (word, &index) in indices.iter().take(count).enumerate() {
                totals[index] = totals[index].wrapping_add(u64::from_le_bytes(
                    bytes[word * 8..word * 8 + 8].try_into().unwrap(),
                ));
            }
            drop(bytes);
            buffer.unmap();
        }
        Ok(if kind == 0 {
            vec![totals[0]]
        } else {
            (0..11)
                .filter(|i| scope.mask & (1 << i) != 0)
                .map(|i| totals[i])
                .collect()
        })
    }
}
impl Queue {
    pub(super) fn query_sets(&self, compute: bool) -> Result<PassQueries> {
        let s = self
            .context
            .queries
            .lock()
            .map_err(|_| Error::InvalidState)?;
        let device = self.context.raw_device();
        Ok(PassQueries {
            occlusion: if !compute && s.active[0].is_some() {
                Some(device.create_query_set(&raw::QuerySetDescriptor {
                    label: Some("SGFX occlusion"),
                    ty: raw::QueryType::Occlusion,
                    count: 1,
                }))
            } else {
                None
            },
            statistics: if s.active[1].is_some() {
                Some(device.create_query_set(&raw::QuerySetDescriptor {
                    label: Some("SGFX statistics"),
                    ty: raw::QueryType::PipelineStatistics(raw::PipelineStatisticsTypes::all()),
                    count: 1,
                }))
            } else {
                None
            },
        })
    }
    pub(super) fn resolve_queries(
        &self,
        encoder: &mut raw::CommandEncoder,
        sets: PassQueries,
    ) -> Result<()> {
        let mut s = self
            .context
            .queries
            .lock()
            .map_err(|_| Error::InvalidState)?;
        for (kind, set) in [sets.occlusion, sets.statistics].into_iter().enumerate() {
            if let Some(set) = set {
                let count = if kind == 0 { 1 } else { 5 };
                let size = count * 8;
                let device = self.context.raw_device();
                let resolve = device.create_buffer(&raw::BufferDescriptor {
                    label: None,
                    size,
                    usage: raw::BufferUsages::QUERY_RESOLVE | raw::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                });
                let output = device.create_buffer(&raw::BufferDescriptor {
                    label: None,
                    size,
                    usage: raw::BufferUsages::MAP_READ | raw::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                encoder.resolve_query_set(&set, 0..1, &resolve, 0);
                encoder.copy_buffer_to_buffer(&resolve, 0, &output, 0, size);
                s.active[kind]
                    .as_mut()
                    .ok_or(Error::InvalidState)?
                    .pending
                    .push((output, count as usize));
            }
        }
        Ok(())
    }
    pub(super) fn assembly_query(
        &self,
        vertices: u32,
        instances: u32,
        topology: ir::PrimitiveTopology,
    ) -> Result<()> {
        let mut s = self
            .context
            .queries
            .lock()
            .map_err(|_| Error::InvalidState)?;
        if let Some(q) = &mut s.active[1] {
            // These two IA counters are exact submitted work counts. Shader and
            // raster counters above come from hardware. No GS/tessellation stage
            // is exposed by this backend; those four stage counters stay zero.
            let primitives = match topology {
                ir::PrimitiveTopology::TriangleList => vertices / 3,
                ir::PrimitiveTopology::TriangleStrip | ir::PrimitiveTopology::TriangleFan => {
                    vertices.saturating_sub(2)
                }
                ir::PrimitiveTopology::LineList => vertices / 2,
                ir::PrimitiveTopology::LineStrip => vertices.saturating_sub(1),
                ir::PrimitiveTopology::PointList => vertices,
            };
            q.assembly[0] = q.assembly[0].wrapping_add(u64::from(vertices) * u64::from(instances));
            q.assembly[1] =
                q.assembly[1].wrapping_add(u64::from(primitives) * u64::from(instances));
        }
        Ok(())
    }
}
