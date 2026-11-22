//! Convert Scarlet's stateful access unit formats to VideoToolbox samples.

use super::{MAX_INPUT, toolbox::Codec};

#[derive(Default)]
pub(super) struct ParameterSets {
    groups: [Vec<Vec<u8>>; 3],
}

impl ParameterSets {
    pub(super) fn prepare(
        &mut self,
        codec: Codec,
        coded: &[u8],
    ) -> Result<(Vec<u8>, bool), &'static str> {
        let first = start_code(coded).ok_or("Expected Annex B video data")?;
        if coded[..first].iter().any(|&byte| byte != 0) {
            return Err("Invalid bytes before Annex B start code");
        }

        let mut parameter_sets: [Vec<Vec<u8>>; 3] = Default::default();
        let mut sample = Vec::new();
        let mut rest = &coded[first + 3..];
        let mut has_picture = false;
        loop {
            let end = start_code(rest).unwrap_or(rest.len());
            let mut nal = &rest[..end];
            while let Some((0, prefix)) = nal.split_last() {
                nal = prefix;
            }
            let header = *nal.first().ok_or("Empty Annex B NAL unit")?;
            if header & 0x80 != 0 || (codec == Codec::Hevc && nal.len() < 2) {
                return Err("Invalid Annex B NAL header");
            }
            let (group, picture) = match codec {
                Codec::H264 => {
                    let kind = header & 0x1f;
                    (
                        (7..=8).contains(&kind).then(|| usize::from(kind - 7)),
                        (1..=5).contains(&kind),
                    )
                }
                Codec::Hevc => {
                    let kind = (header >> 1) & 0x3f;
                    (
                        (32..=34).contains(&kind).then(|| usize::from(kind - 32)),
                        kind <= 31,
                    )
                }
                Codec::Av1 => return Err("AV1 access units require an av1C record"),
            };
            if let Some(group) = group {
                // The container frontend prepends all parameter sets of a
                // given kind. Keep prior groups when a raw stream omits them.
                if !parameter_sets[group].iter().any(|previous| previous == nal) {
                    parameter_sets[group].push(nal.to_vec());
                }
            } else {
                has_picture |= picture;
                let length = u32::try_from(nal.len()).map_err(|_| "Video NAL is too large")?;
                sample.extend_from_slice(&length.to_be_bytes());
                sample.extend_from_slice(nal);
            }
            if end == rest.len() {
                break;
            }
            rest = &rest[end + 3..];
        }

        let retained: usize = self
            .groups
            .iter()
            .zip(&parameter_sets)
            .map(|(old, new)| {
                let sets = if new.is_empty() { old } else { new };
                sets.iter().map(Vec::len).sum::<usize>()
            })
            .sum();
        if retained > MAX_INPUT {
            return Err("Video parameter sets exceed the input buffer capacity");
        }

        let mut changed = false;
        for (sets, incoming) in self.groups.iter_mut().zip(parameter_sets) {
            if !incoming.is_empty() && *sets != incoming {
                *sets = incoming;
                changed = true;
            }
        }
        if !has_picture {
            sample.clear();
        }
        Ok((sample, changed))
    }

    pub(super) fn for_format(&self, codec: Codec) -> Result<Vec<&[u8]>, &'static str> {
        let groups = match codec {
            Codec::H264 => &self.groups[..2],
            Codec::Hevc => &self.groups[..],
            Codec::Av1 => return Err("AV1 does not use Annex B parameter sets"),
        };
        if groups.iter().any(Vec::is_empty) {
            return Err("VideoToolbox is missing video parameter sets");
        }
        Ok(groups.iter().flatten().map(Vec::as_slice).collect())
    }
}

fn start_code(bytes: &[u8]) -> Option<usize> {
    bytes.windows(3).position(|window| window == [0, 0, 1])
}

pub(super) struct Av1Sample<'a> {
    pub width: i32,
    pub height: i32,
    pub config: &'a [u8],
    pub sample: &'a [u8],
}

pub(super) fn av1(coded: &[u8]) -> Result<Av1Sample<'_>, &'static str> {
    let Some((header, payload)) = coded.split_at_checked(20) else {
        return Err("Truncated Scarlet AV1 access unit");
    };
    if &header[..4] != b"SVA1" {
        return Err("Invalid Scarlet AV1 access unit");
    }
    let field = |offset: usize| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    let width = i32::try_from(field(4)).map_err(|_| "Invalid AV1 width")?;
    let height = i32::try_from(field(8)).map_err(|_| "Invalid AV1 height")?;
    let Some((config, sample)) = payload.split_at_checked(field(12) as usize) else {
        return Err("Truncated av1C record");
    };
    if width == 0
        || height == 0
        || config.len() < 4
        || config[0] != 0x81
        || sample.is_empty()
        || sample.len() != field(16) as usize
    {
        return Err("Invalid AV1 sample or configuration");
    }
    Ok(Av1Sample {
        width,
        height,
        config,
        sample,
    })
}
