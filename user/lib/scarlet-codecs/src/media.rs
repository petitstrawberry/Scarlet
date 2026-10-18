//! Bounded, platform-independent software media primitives.
//!
//! Network access, playback clocks, output queues and rendering belong to the
//! caller. MP4 samples are pulled one at a time; fragment boundaries never reset
//! their media timestamps. Only 8-bit 4:2:0 AVC and AAC-LC mono/stereo are supported.
#[cfg(feature = "software-h264")]
use rust_h264::{
    decoder::OrderedDecoder,
    nal::{self, NalUnit, NalUnitType},
};
use std::{boxed::Box, collections::VecDeque, format, string::String, vec::Vec};
use symphonia_core::{
    codecs::{
        audio::{
            AudioCodecParameters, AudioDecoder as DecodeAudio, AudioDecoderOptions,
            well_known::CODEC_ID_AAC,
        },
        video::well_known::{CODEC_ID_H264, extra_data::VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG},
    },
    formats::{FormatOptions, FormatReader},
    io::{MediaSourceStream, ReadBytes},
    packet::PacketRef,
    units::{Duration, TimeBase, Timestamp},
};
use symphonia_format_isomp4::IsoMp4Reader;
pub use symphonia_format_isomp4::cenc::{SampleEncryption, SubsampleEncryption};

pub const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SAMPLE_BYTES: usize = 4 * 1024 * 1024;
const MAX_INIT_BYTES: usize = 1024 * 1024;
const MAX_SAMPLES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    InvalidData(&'static str),
    Unsupported(&'static str),
    BufferFull,
    Decode(String),
}
impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MediaError {}
type Result<T> = std::result::Result<T, MediaError>;
fn decode_err(e: impl std::fmt::Display) -> MediaError {
    MediaError::Decode(format!("{e}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Aac,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub id: u32,
    pub codec: Codec,
    pub extradata: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
    pub timescale: u32,
    pub duration_us: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct Sample {
    /// Present until the caller has decrypted this encoded payload.
    pub encryption: Option<SampleEncryption>,
    pub track_id: u32,
    pub data: Vec<u8>,
    pub pts_us: i64,
    pub dts_us: i64,
    pub duration_us: u64,
    pub keyframe: bool,
}
#[derive(Clone, Debug)]
pub enum ParserEvent {
    /// Complete PSSH boxes for the EME cenc initialization-data event.
    InitializationData(Vec<u8>),
    Track(Track),
    Sample(Sample),
}

fn micros(value: i128, time: TimeBase) -> Result<i128> {
    value
        .checked_mul(i128::from(time.numer.get()))
        .and_then(|v| v.checked_mul(1_000_000))
        .map(|v| v / i128::from(time.denom.get()))
        .ok_or(MediaError::InvalidData("timestamp overflow"))
}
fn signed_us(value: i64, time: TimeBase) -> Result<i64> {
    i64::try_from(micros(i128::from(value), time)?)
        .map_err(|_| MediaError::InvalidData("timestamp overflow"))
}
fn duration_us(value: u64, time: TimeBase) -> Result<u64> {
    u64::try_from(micros(i128::from(value), time)?)
        .map_err(|_| MediaError::InvalidData("duration overflow"))
}
fn reader(bytes: Vec<u8>) -> Result<IsoMp4Reader<'static>> {
    let mut input =
        MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    // The public format constructor follows the probe convention: the first
    // four bytes have already been consumed and are rewound internally.
    input.read_be_u32().map_err(decode_err)?;
    IsoMp4Reader::try_new(input, FormatOptions::default()).map_err(decode_err)
}

/// Incrementally accepts bounded complete MP4 boxes. fMP4 initialization and
/// each moof/mdat pair are independently demuxed by Symphonia's shared parser.
/// Ordinary MP4 is accepted up to the buffer bound; non-faststart files require
/// finish(). Call next_event() until None after every accepted input chunk.
/// BufferFull leaves input unchanged; drain events before retrying the chunk.
#[derive(Default)]
pub struct Mp4Parser {
    pending: Vec<u8>,
    init: Vec<u8>,
    fragment: Vec<u8>,
    tracks: Vec<(Track, TimeBase)>,
    events: VecDeque<ParserEvent>,
    initialization_data: Vec<Vec<u8>>,
    active: Option<IsoMp4Reader<'static>>,
    active_bytes: usize,
    sample_count: usize,
    have_moov: bool,
    slow_file: bool,
    finished: bool,
}
impl Mp4Parser {
    pub fn new() -> Self {
        Self::default()
    }
    /// Discard an incomplete append/fragment while retaining an accepted init
    /// segment. Suitable for MSE abort/resetParserState; use new() for a new URL.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.fragment.clear();
        self.events.clear();
        self.active = None;
        self.active_bytes = 0;
        self.sample_count = 0;
        self.finished = false;
        if self.slow_file || !self.have_moov {
            self.init.clear();
            self.tracks.clear();
            self.have_moov = false;
        }
        self.slow_file = false;
    }
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if self.finished {
            return Err(MediaError::InvalidData("append after finish"));
        }
        let total = self
            .pending
            .len()
            .saturating_add(self.init.len())
            .saturating_add(self.fragment.len())
            .saturating_add(self.active_bytes);
        if bytes.len() > MAX_BUFFER_BYTES.saturating_sub(total) {
            return Err(MediaError::BufferFull);
        }
        self.pending.extend_from_slice(bytes);
        Ok(())
    }
    pub fn finish(&mut self) -> Result<()> {
        self.finished = true;
        Ok(())
    }
    pub fn next_event(&mut self) -> Result<Option<ParserEvent>> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            if let Some(active) = &mut self.active {
                if let Some(packet) = active.next_packet().map_err(decode_err)? {
                    self.sample_count += 1;
                    if self.sample_count > MAX_SAMPLES {
                        return Err(MediaError::Unsupported("too many samples in one segment"));
                    }
                    if packet.data.len() > MAX_SAMPLE_BYTES {
                        return Err(MediaError::Unsupported("oversized sample"));
                    }
                    let Some((track, time)) = self
                        .tracks
                        .iter()
                        .find(|(track, _)| track.id == packet.track_id)
                    else {
                        continue;
                    };
                    let encryption = active.packet_encryption().cloned();
                    let keyframe = if encryption.is_some() {
                        active.packet_is_sync()
                    } else if track.codec == Codec::H264 {
                        let length_size = avcc_length_size(&track.extradata)?;
                        avcc_nals(&packet.data, length_size)?
                            .iter()
                            .any(|nal| nal[0] & 31 == 5)
                    } else {
                        true
                    };
                    return Ok(Some(ParserEvent::Sample(Sample {
                        encryption,
                        track_id: track.id,
                        data: packet.data.into_vec(),
                        pts_us: signed_us(packet.pts.get(), *time)?,
                        dts_us: signed_us(packet.dts.get(), *time)?,
                        duration_us: duration_us(packet.dur.get(), *time)?,
                        keyframe,
                    })));
                }
                self.active = None;
                self.active_bytes = 0;
            }
            if self.pending.is_empty() {
                if self.finished && !self.fragment.is_empty() {
                    return Err(MediaError::InvalidData("fragment missing mdat"));
                }
                if self.finished && self.slow_file {
                    let bytes = std::mem::take(&mut self.init);
                    self.slow_file = false;
                    self.open(bytes)?;
                    continue;
                }
                return Ok(None);
            }
            let Some((length, header)) = box_header(&self.pending, self.finished)? else {
                if self.finished {
                    return Err(MediaError::InvalidData("truncated MP4 box"));
                }
                return Ok(None);
            };
            if length > self.pending.len() {
                if self.finished {
                    return Err(MediaError::InvalidData("truncated MP4 box"));
                }
                return Ok(None);
            }
            let kind: [u8; 4] = self.pending[4..8].try_into().unwrap();
            validate_boxes(&self.pending[..length], 0)?;
            let bytes: Vec<u8> = self.pending.drain(..length).collect();
            match &kind {
                b"moof" if self.have_moov && !self.slow_file => {
                    if !self.fragment.is_empty() {
                        return Err(MediaError::InvalidData("fragment missing mdat"));
                    }
                    self.fragment = bytes;
                }
                b"mdat" if self.have_moov && !self.slow_file => {
                    let mut data = self.init.clone();
                    data.append(&mut self.fragment);
                    data.extend_from_slice(&bytes);
                    self.open(data)?;
                }
                b"moov" if !self.slow_file => {
                    if self.have_moov {
                        self.init.clear();
                        self.tracks.clear();
                        self.initialization_data.clear();
                    }
                    self.init.extend_from_slice(&bytes);
                    if bytes.len() > MAX_INIT_BYTES {
                        return Err(MediaError::Unsupported("oversized initialization"));
                    }
                    self.have_moov = true;
                    let r = reader(self.init.clone())?;
                    self.update_tracks(&r)?;
                }
                b"mdat" => {
                    self.slow_file = true;
                    self.init.extend_from_slice(&bytes);
                }
                b"moov" => {
                    self.have_moov = true;
                    self.init.extend_from_slice(&bytes);
                }
                b"ftyp" if self.have_moov && !self.slow_file => {
                    // A new MSE initialization segment starts a fresh track set.
                    self.have_moov = false;
                    self.init = bytes;
                    self.tracks.clear();
                    self.initialization_data.clear();
                }
                _ => {
                    // Trailer indices are not part of the next fragment. Prefix
                    // boxes retain their positions for ordinary MP4 stco/co64.
                    if !self.have_moov || self.slow_file {
                        self.init.extend_from_slice(&bytes);
                    } else if !self.fragment.is_empty() {
                        self.fragment.extend_from_slice(&bytes);
                    } else if kind == *b"free" || kind == *b"skip" {
                        self.init.extend_from_slice(&bytes);
                    }
                }
            }
            let _ = header;
        }
    }
    fn open(&mut self, bytes: Vec<u8>) -> Result<()> {
        let size = bytes.len();
        let r = reader(bytes)?;
        self.update_tracks(&r)?;
        self.active = Some(r);
        self.active_bytes = size;
        self.sample_count = 0;
        Ok(())
    }
    fn update_tracks(&mut self, r: &IsoMp4Reader<'_>) -> Result<()> {
        for header in r.protection_headers() {
            if !self.initialization_data.iter().any(|previous| previous.as_slice() == header.as_ref()) {
                self.initialization_data.push(header.to_vec());
                self.events.push_back(ParserEvent::InitializationData(header.to_vec()));
            }
        }
        if r.tracks().len() > 8 {
            return Err(MediaError::Unsupported("too many tracks"));
        }
        for t in r.tracks() {
            let Some(params) = &t.codec_params else {
                continue;
            };
            let time = t
                .time_base
                .ok_or(MediaError::InvalidData("missing track timebase"))?;
            if time.numer.get() != 1 {
                return Err(MediaError::Unsupported("non-integral track timescale"));
            }
            let mut track = Track {
                id: t.id,
                codec: Codec::H264,
                extradata: Vec::new(),
                width: 0,
                height: 0,
                sample_rate: 0,
                channels: 0,
                timescale: time.denom.get(),
                duration_us: t.duration.map(|d| duration_us(d.get(), time)).transpose()?,
            };
            if let Some(v) = params.video() {
                if v.codec != CODEC_ID_H264 {
                    continue;
                }
                let data = v
                    .extra_data
                    .iter()
                    .find(|d| d.id == VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG)
                    .ok_or(MediaError::InvalidData("missing avcC"))?;
                track.extradata = data.data.to_vec();
                track.width = u32::from(v.width.unwrap_or(0));
                track.height = u32::from(v.height.unwrap_or(0));
            } else if let Some(a) = params.audio() {
                if a.codec != CODEC_ID_AAC {
                    continue;
                }
                track.codec = Codec::Aac;
                track.extradata = a.extra_data.as_deref().unwrap_or_default().to_vec();
                track.sample_rate = a.sample_rate.unwrap_or(0);
                track.channels = a.channels.as_ref().map_or(0, |c| c.count() as u16);
            } else {
                continue;
            }
            if track.extradata.len() > 65536 {
                return Err(MediaError::Unsupported("oversized codec configuration"));
            }
            if let Some(existing) = self.tracks.iter_mut().find(|(old, _)| old.id == track.id) {
                // Segment index duration may vary without changing codec config.
                if existing.0.codec != track.codec
                    || existing.0.extradata != track.extradata
                    || existing.1 != time
                {
                    *existing = (track.clone(), time);
                    self.events.push_back(ParserEvent::Track(track));
                }
            } else {
                self.tracks.push((track.clone(), time));
                self.events.push_back(ParserEvent::Track(track));
            }
        }
        if self.tracks.is_empty() {
            return Err(MediaError::Unsupported("no supported AVC/AAC tracks"));
        }
        Ok(())
    }
}
fn box_header(bytes: &[u8], eof: bool) -> Result<Option<(usize, usize)>> {
    if bytes.len() < 8 {
        return Ok(None);
    }
    let size = u32::from_be_bytes(bytes[..4].try_into().unwrap());
    let (size, header) = match size {
        0 if eof => (bytes.len() as u64, 8),
        0 => return Ok(None),
        1 => {
            if bytes.len() < 16 {
                return Ok(None);
            }
            (u64::from_be_bytes(bytes[8..16].try_into().unwrap()), 16)
        }
        n => (u64::from(n), 8),
    };
    if size < header as u64 {
        return Err(MediaError::InvalidData("invalid MP4 box size"));
    }
    if size > MAX_BUFFER_BYTES as u64 {
        return Err(MediaError::Unsupported("oversized MP4 box"));
    }
    Ok(Some((size as usize, header)))
}
// This is a bounded structural guard, not a second sample-table parser.
// Symphonia retains ownership of all MP4 field interpretation and demuxing.
fn validate_boxes(mut bytes: &[u8], depth: usize) -> Result<()> {
    if depth > 12 {
        return Err(MediaError::InvalidData("excessive MP4 nesting"));
    }
    while !bytes.is_empty() {
        let (size, h) =
            box_header(bytes, true)?.ok_or(MediaError::InvalidData("truncated box header"))?;
        if size > bytes.len() {
            return Err(MediaError::InvalidData("truncated nested box"));
        }
        let kind = &bytes[4..8];
        let payload = &bytes[h..size];
        match kind {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"mvex" | b"moof" | b"traf"
            | b"edts" => validate_boxes(payload, depth + 1)?,
            b"tfhd" => {
                if payload.len() < 8 {
                    return Err(MediaError::InvalidData("truncated tfhd"));
                }
                if payload[3] & 1 != 0 {
                    return Err(MediaError::Unsupported("absolute fragment base offset"));
                }
                let flags = u32::from_be_bytes(payload[..4].try_into().unwrap()) & 0x00ff_ffff;
                let mut offset: usize = 8;
                if flags & 2 != 0 {
                    offset += 4;
                }
                if flags & 8 != 0 {
                    offset += 4;
                }
                if flags & 16 != 0 {
                    check_sample_size(read_u32(payload, offset)?)?;
                }
            }
            b"trun" => {
                if payload.len() < 8 {
                    return Err(MediaError::InvalidData("truncated trun"));
                }
                let flags = read_u32(payload, 0)? & 0x00ff_ffff;
                let count = read_u32(payload, 4)? as usize;
                if count > MAX_SAMPLES {
                    return Err(MediaError::Unsupported("oversized sample table"));
                }
                let mut offset: usize = 8;
                if flags & 1 != 0 {
                    offset += 4;
                }
                if flags & 4 != 0 {
                    offset += 4;
                }
                let fields = (flags & 0xf00).count_ones() as usize * 4;
                let end = offset
                    .checked_add(count * fields)
                    .ok_or(MediaError::InvalidData("trun size overflow"))?;
                if end > payload.len() {
                    return Err(MediaError::InvalidData("truncated sample records"));
                }
                if flags & 0x200 != 0 {
                    let size_offset = if flags & 0x100 != 0 { 4 } else { 0 };
                    for sample in 0..count {
                        check_sample_size(read_u32(
                            payload,
                            offset + sample * fields + size_offset,
                        )?)?;
                    }
                }
            }
            b"stsd" | b"stts" | b"ctts" | b"stsc" | b"stco" | b"co64" | b"stss" => {
                if payload.len() < 8 {
                    return Err(MediaError::InvalidData("truncated sample table"));
                }
                let count = u32::from_be_bytes(payload[4..8].try_into().unwrap());
                if count as usize > MAX_SAMPLES {
                    return Err(MediaError::Unsupported("oversized sample table"));
                }
            }
            b"stsz" => {
                if payload.len() < 12 {
                    return Err(MediaError::InvalidData("truncated stsz"));
                }
                let count = read_u32(payload, 8)? as usize;
                if count > MAX_SAMPLES {
                    return Err(MediaError::Unsupported("oversized sample table"));
                }
                let size = read_u32(payload, 4)?;
                check_sample_size(size)?;
                if size == 0 {
                    for sample in 0..count {
                        check_sample_size(read_u32(payload, 12 + sample * 4)?)?;
                    }
                }
            }
            _ => (),
        }
        bytes = &bytes[size..];
    }
    Ok(())
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or(MediaError::InvalidData("truncated MP4 field"))?;
    Ok(u32::from_be_bytes(bytes.try_into().unwrap()))
}
fn check_sample_size(size: u32) -> Result<()> {
    if size as usize > MAX_SAMPLE_BYTES {
        Err(MediaError::Unsupported("oversized encoded sample"))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorInfo {
    pub matrix: u8,
    pub primaries: u8,
    pub transfer: u8,
    pub full_range: bool,
    pub chroma_location: u8,
}
/// Validated AVC configuration for native decoders. This converts MP4 framing
/// only; it neither decodes pictures nor opens a platform video device.
pub struct AvcConfiguration {
    track_id: u32,
    length_size: usize,
    parameter_sets: Vec<Vec<u8>>,
    color: ColorInfo,
    max_reorder_frames: usize,
    width: u32,
    height: u32,
}
impl AvcConfiguration {
    pub fn new(track: &Track) -> Result<Self> {
        if track.codec != Codec::H264 {
            return Err(MediaError::InvalidData("not an AVC track"));
        }
        let data = &track.extradata;
        let length_size = avcc_length_size(data)?;
        let mut offset = 6;
        let sps_count = usize::from(data[5] & 31);
        if sps_count == 0 {
            return Err(MediaError::InvalidData("missing AVC SPS"));
        }
        if sps_count != 1 {
            return Err(MediaError::Unsupported("multiple AVC SPS configurations"));
        }
        let mut parameter_sets = Vec::new();
        let mut info = None;
        for _ in 0..sps_count {
            let bytes = read_avcc_parameter(data, &mut offset)?;
            info = Some(native_sps_info(bytes)?);
            parameter_sets.push(bytes.to_vec());
        }
        let pps_count = usize::from(
            *data
                .get(offset)
                .ok_or(MediaError::InvalidData("missing AVC PPS count"))?,
        );
        offset += 1;
        if pps_count == 0 {
            return Err(MediaError::InvalidData("missing AVC PPS"));
        }
        if pps_count != 1 {
            return Err(MediaError::Unsupported("multiple AVC PPS configurations"));
        }
        for _ in 0..pps_count {
            let bytes = read_avcc_parameter(data, &mut offset)?;
            if bytes[0] & 31 != 8 {
                return Err(MediaError::InvalidData("invalid AVC PPS type"));
            }
            crate::h264::parameter_set_id(bytes).map_err(decode_err)?;
            parameter_sets.push(bytes.to_vec());
        }
        if crate::h264::pps_sps_id(&parameter_sets[1]).map_err(decode_err)?
            != crate::h264::parameter_set_id(&parameter_sets[0]).map_err(decode_err)?
        {
            return Err(MediaError::InvalidData("AVC PPS references missing SPS"));
        }
        let (color, depth, width, height) = info.unwrap();
        Ok(Self {
            track_id: track.id,
            length_size,
            parameter_sets,
            color,
            max_reorder_frames: depth,
            width,
            height,
        })
    }
    pub fn color(&self) -> ColorInfo {
        self.color
    }
    pub fn max_reorder_frames(&self) -> usize {
        self.max_reorder_frames
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    /// Prepend avcC parameter sets to a complete access unit, preserving raw
    /// EBSP bytes. Validate in-band SPS before native decoder allocation.
    pub fn annex_b(&mut self, sample: &Sample) -> Result<Vec<u8>> {
        if sample.encryption.is_some() { return Err(MediaError::InvalidData("encrypted AVC sample requires decryption")); }
        if sample.track_id != self.track_id {
            return Err(MediaError::InvalidData("wrong AVC track"));
        }
        let nals = avcc_nals(&sample.data, self.length_size)?;
        let mut info = None;
        let mut replacements = Vec::new();
        let mut pictures = 0;
        for nal in &nals {
            if nal[0] & 31 == 7 {
                info = Some(native_sps_info(nal)?);
            }
            if matches!(nal[0] & 31, 7 | 8) {
                let id = crate::h264::parameter_set_id(nal).map_err(decode_err)?;
                let known = self.parameter_sets.iter().any(|old| {
                    old[0] & 31 == nal[0] & 31
                        && crate::h264::parameter_set_id(old).ok() == Some(id)
                });
                if !known {
                    return Err(MediaError::Unsupported(
                        "multiple AVC parameter-set identifiers",
                    ));
                }
                if nal[0] & 31 == 8
                    && crate::h264::pps_sps_id(nal).map_err(decode_err)?
                        != crate::h264::parameter_set_id(&self.parameter_sets[0])
                            .map_err(decode_err)?
                {
                    return Err(MediaError::InvalidData("AVC PPS references missing SPS"));
                }
                replacements.push((nal[0] & 31, id, *nal));
            }
            if matches!(nal[0] & 31, 1 | 5) {
                if crate::h264::slice_pps_id(nal).map_err(decode_err)?
                    != crate::h264::parameter_set_id(&self.parameter_sets[1]).map_err(decode_err)?
                {
                    return Err(MediaError::InvalidData("AVC slice references missing PPS"));
                }
                let first = nal
                    .get(1)
                    .ok_or(MediaError::InvalidData("empty AVC slice"))?;
                if first & 0x80 != 0 {
                    pictures += 1;
                }
            }
        }
        if pictures != 1 {
            return Err(MediaError::InvalidData(
                "AVC access unit requires one coded picture",
            ));
        }
        // New parameter sets persist into later access units. Retaining an old
        // avcC prefix would otherwise overwrite the in-band configuration again.
        let mut parameter_sets = self.parameter_sets.clone();
        for (kind, id, nal) in replacements {
            let existing = parameter_sets.iter().position(|old| {
                old[0] & 31 == kind && crate::h264::parameter_set_id(old).ok() == Some(id)
            });
            if let Some(index) = existing {
                parameter_sets[index] = nal.to_vec();
            } else {
                parameter_sets.push(nal.to_vec());
            }
        }
        if parameter_sets.len() > 287 || parameter_sets.iter().map(Vec::len).sum::<usize>() > 65536
        {
            return Err(MediaError::Unsupported("oversized AVC configuration"));
        }
        let mut len = 0usize;
        for n in parameter_sets
            .iter()
            .map(|n| n.as_slice())
            .chain(nals.iter().copied())
        {
            len = len
                .checked_add(4)
                .and_then(|v| v.checked_add(n.len()))
                .ok_or(MediaError::InvalidData("Annex B size overflow"))?;
        }
        // Config and length-prefix expansion are bounded separately from the
        // input sample so a maximum-size valid AVCC sample remains accepted.
        if len > MAX_SAMPLE_BYTES + 65536 + 4 * 256 {
            return Err(MediaError::Unsupported("oversized Annex B access unit"));
        }
        let mut out = Vec::with_capacity(len);
        for n in parameter_sets
            .iter()
            .map(|n| n.as_slice())
            .chain(nals.iter().copied())
        {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(n);
        }
        self.parameter_sets = parameter_sets;
        if let Some((color, depth, width, height)) = info {
            self.color = color;
            self.max_reorder_frames = depth;
            self.width = width;
            self.height = height;
        }
        Ok(out)
    }
}
fn avcc_length_size(data: &[u8]) -> Result<usize> {
    if data.len() < 7 || data.len() > 65536 || data[0] != 1 {
        return Err(MediaError::InvalidData("invalid avcC configuration"));
    }
    let length = usize::from(data[4] & 3) + 1;
    if !matches!(length, 1 | 2 | 4) {
        return Err(MediaError::InvalidData("invalid AVC length size"));
    }
    Ok(length)
}
fn read_avcc_parameter<'a>(data: &'a [u8], offset: &mut usize) -> Result<&'a [u8]> {
    let len = data
        .get(*offset..*offset + 2)
        .ok_or(MediaError::InvalidData("truncated avcC parameter length"))?;
    let len = usize::from(u16::from_be_bytes(len.try_into().unwrap()));
    *offset += 2;
    let nal = data
        .get(*offset..*offset + len)
        .ok_or(MediaError::InvalidData("truncated avcC parameter"))?;
    *offset += len;
    if nal.is_empty() || nal[0] & 0x80 != 0 {
        return Err(MediaError::InvalidData("invalid avcC parameter header"));
    }
    Ok(nal)
}
fn avcc_nals(mut data: &[u8], length_size: usize) -> Result<Vec<&[u8]>> {
    validate_avcc(data, length_size)?;
    let mut nals = Vec::new();
    while !data.is_empty() {
        let len = data[..length_size]
            .iter()
            .fold(0usize, |n, b| (n << 8) | usize::from(*b));
        data = &data[length_size..];
        nals.push(&data[..len]);
        data = &data[len..];
    }
    Ok(nals)
}
fn native_sps_info(nal: &[u8]) -> Result<(ColorInfo, usize, u32, u32)> {
    use crate::h264::{SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY, inspect_sps};
    let info = inspect_sps(nal).map_err(decode_err)?;
    let s = info.sps;
    if !matches!(s.profile_idc, 66 | 77 | 100)
        || s.chroma_format_idc != 1
        || s.bit_depth_luma_minus8 != 0
        || s.bit_depth_chroma_minus8 != 0
        || s.flags & SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY == 0
    {
        return Err(MediaError::Unsupported(
            "AVC requires progressive Baseline/Main/High 8-bit 4:2:0",
        ));
    }
    let coded_width = (u32::from(s.pic_width_in_mbs_minus1) + 1)
        .checked_mul(16)
        .ok_or(MediaError::InvalidData("AVC width overflow"))?;
    let coded_height = (u32::from(s.pic_height_in_map_units_minus1) + 1)
        .checked_mul(16)
        .ok_or(MediaError::InvalidData("AVC height overflow"))?;
    let crop_x = s
        .frame_crop_left_offset
        .checked_add(s.frame_crop_right_offset)
        .and_then(|v| v.checked_mul(2))
        .ok_or(MediaError::InvalidData("AVC crop overflow"))?;
    let crop_y = s
        .frame_crop_top_offset
        .checked_add(s.frame_crop_bottom_offset)
        .and_then(|v| v.checked_mul(2))
        .ok_or(MediaError::InvalidData("AVC crop overflow"))?;
    let width = coded_width
        .checked_sub(crop_x)
        .ok_or(MediaError::InvalidData("invalid AVC crop"))?;
    let height = coded_height
        .checked_sub(crop_y)
        .ok_or(MediaError::InvalidData("invalid AVC crop"))?;
    let depth = match info.max_reorder_frames {
        Some(depth) => depth,
        None if s.profile_idc == 66 => 0,
        None => {
            return Err(MediaError::Unsupported(
                "AVC reorder depth is not signalled",
            ));
        }
    };
    if width == 0
        || height == 0
        || coded_width > 3840
        || coded_height > 2160
        || coded_width
            .checked_mul(coded_height)
            .is_none_or(|v| v > 3840 * 2160)
        || s.max_num_ref_frames > 8
        || depth > 2
    {
        return Err(MediaError::Unsupported(
            "AVC dimensions, references or reorder depth exceed bounds",
        ));
    }
    let c = info.color;
    Ok((
        ColorInfo {
            matrix: c.matrix,
            primaries: c.primaries,
            transfer: c.transfer,
            full_range: c.full_range,
            chroma_location: c.chroma_location as u8,
        },
        depth as usize,
        width,
        height,
    ))
}

#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub strides: [usize; 3],
    pub pts_us: i64,
    pub duration_us: u64,
    pub color: ColorInfo,
}
#[derive(Clone, Copy)]
#[cfg(feature = "software-h264")]
struct PictureMeta {
    pts_us: i64,
    duration_us: u64,
    color: ColorInfo,
}
#[cfg(feature = "software-h264")]
pub struct VideoDecoder {
    decoder: OrderedDecoder<PictureMeta>,
    track: Track,
    length_size: usize,
    color: ColorInfo,
}
#[cfg(feature = "software-h264")]
impl VideoDecoder {
    pub fn new(track: &Track) -> Result<Self> {
        if track.codec != Codec::H264 {
            return Err(MediaError::InvalidData("not an AVC track"));
        }
        if track.extradata.len() > 65536 {
            return Err(MediaError::Unsupported("oversized AVC configuration"));
        }
        let config = nal::parse_avcc_config(&track.extradata).map_err(MediaError::InvalidData)?;
        if config.sps_nals.is_empty() || config.pps_nals.is_empty() {
            return Err(MediaError::InvalidData("missing AVC parameter sets"));
        }
        let mut result = Self {
            decoder: OrderedDecoder::with_max_depth(2),
            track: track.clone(),
            length_size: config.length_size,
            color: ColorInfo {
                matrix: 2,
                primaries: 2,
                transfer: 2,
                full_range: false,
                chroma_location: 0,
            },
        };
        for n in config.sps_nals.iter().chain(config.pps_nals.iter()) {
            result.feed_config(n)?;
        }
        Ok(result)
    }
    fn feed_config(&mut self, n: &NalUnit<'_>) -> Result<()> {
        if n.nal_unit_type == NalUnitType::Sps {
            self.color = check_sps(n)?;
        }
        self.decoder
            .decode_nal_with_meta(
                n,
                PictureMeta {
                    pts_us: 0,
                    duration_us: 0,
                    color: self.color,
                },
            )
            .map_err(decode_err)?;
        Ok(())
    }
    /// Reset after seeking/discontinuity; reinstalls the original codec config.
    pub fn reset(&mut self) -> Result<()> {
        *self = Self::new(&self.track)?;
        Ok(())
    }
    /// At most three frames are returned, including at a new IDR boundary.
    pub fn push(&mut self, sample: &Sample) -> Result<Vec<VideoFrame>> {
        if sample.track_id != self.track.id {
            return Err(MediaError::InvalidData("wrong video track"));
        }
        validate_avcc(&sample.data, self.length_size)?;
        let nals = nal::parse_avcc(&sample.data, self.length_size);
        let mut first_slices = 0;
        for n in &nals {
            if n.nal_unit_type == NalUnitType::Sps {
                check_sps(n)?;
            }
            if matches!(n.nal_unit_type, NalUnitType::Slice | NalUnitType::SliceIdr)
                && first_mb_is_zero(&n.rbsp)?
            {
                first_slices += 1;
            }
        }
        if first_slices > 1 {
            return Err(MediaError::Unsupported("multiple pictures per AVC sample"));
        }
        let mut out = Vec::new();
        for n in &nals {
            if n.nal_unit_type == NalUnitType::Sps {
                self.color = check_sps(n)?;
            }
            let meta = PictureMeta {
                pts_us: sample.pts_us,
                duration_us: sample.duration_us,
                color: self.color,
            };
            out.extend(
                self.decoder
                    .decode_nal_with_meta(n, meta)
                    .map_err(decode_err)?
                    .into_iter()
                    .map(frame),
            );
        }
        if out.len() > 3 {
            return Err(MediaError::Unsupported("excessive AVC frame output"));
        }
        Ok(out)
    }
    pub fn flush(&mut self) -> Result<Vec<VideoFrame>> {
        Ok(self
            .decoder
            .flush_with_meta()
            .into_iter()
            .map(frame)
            .collect())
    }
}
#[cfg(feature = "software-h264")]
fn frame((f, meta): (rust_h264::decoder::Frame, PictureMeta)) -> VideoFrame {
    VideoFrame {
        width: f.width as u32,
        height: f.height as u32,
        strides: [
            f.width as usize,
            f.width.div_ceil(2) as usize,
            f.width.div_ceil(2) as usize,
        ],
        y: f.y,
        u: f.u,
        v: f.v,
        pts_us: meta.pts_us,
        duration_us: meta.duration_us,
        color: meta.color,
    }
}
#[cfg(feature = "software-h264")]
fn check_sps(n: &NalUnit<'_>) -> Result<ColorInfo> {
    let s = nal::parse_sps_info(n).map_err(MediaError::InvalidData)?;
    if !matches!(s.profile, 66 | 77 | 100)
        || s.chroma_format != 1
        || s.bit_depth_luma != 8
        || s.bit_depth_chroma != 8
        || !s.progressive
    {
        return Err(MediaError::Unsupported(
            "AVC requires progressive Baseline/Main/High 8-bit 4:2:0",
        ));
    }
    if s.width == 0
        || s.height == 0
        || s.width > s.coded_width
        || s.height > s.coded_height
        || s.coded_width > 3840
        || s.coded_height > 2160
        || s.coded_width
            .checked_mul(s.coded_height)
            .is_none_or(|pixels| pixels > 3840 * 2160)
        || s.reference_frames > 8
        || s.max_reorder_frames.unwrap_or(2) > 2
    {
        return Err(MediaError::Unsupported(
            "AVC dimensions, references or reorder depth exceed bounds",
        ));
    }
    Ok(ColorInfo {
        matrix: s.matrix,
        primaries: s.primaries,
        transfer: s.transfer,
        full_range: s.full_range,
        chroma_location: s.chroma_location as u8,
    })
}
#[cfg(feature = "software-h264")]
fn first_mb_is_zero(rbsp: &[u8]) -> Result<bool> {
    let byte = rbsp
        .first()
        .ok_or(MediaError::InvalidData("empty AVC slice"))?;
    // Unsigned Exp-Golomb zero is precisely a leading 1 bit.
    Ok(byte & 0x80 != 0)
}
fn validate_avcc(mut data: &[u8], length_size: usize) -> Result<()> {
    if data.len() > MAX_SAMPLE_BYTES {
        return Err(MediaError::Unsupported("oversized AVC sample"));
    }
    if !matches!(length_size, 1 | 2 | 4) {
        return Err(MediaError::InvalidData("invalid AVC length size"));
    }
    let mut count = 0;
    while !data.is_empty() {
        if data.len() < length_size {
            return Err(MediaError::InvalidData("truncated AVC length"));
        }
        let length = data[..length_size]
            .iter()
            .fold(0usize, |n, b| (n << 8) | usize::from(*b));
        data = &data[length_size..];
        if length == 0 || length > data.len() || data[0] & 0x80 != 0 {
            return Err(MediaError::InvalidData("invalid AVC NAL length/header"));
        }
        count += 1;
        if count > 256 {
            return Err(MediaError::Unsupported("too many AVC NAL units"));
        }
        data = &data[length..];
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct AudioFrame {
    pub samples: Vec<i16>,
    pub rate: u32,
    pub channels: u16,
    pub pts_us: i64,
}
pub struct AudioDecoder {
    decoder: symphonia_codec_aac::AacDecoder,
    track: Track,
}
impl AudioDecoder {
    pub fn new(track: &Track) -> Result<Self> {
        use symphonia_core::audio::layouts::{CHANNEL_LAYOUT_MONO, CHANNEL_LAYOUT_STEREO};
        if track.codec != Codec::Aac {
            return Err(MediaError::InvalidData("not an AAC track"));
        }
        if track.extradata.len() < 2
            || track.extradata.len() > 65536
            || track.extradata[0] >> 3 != 2
            || !matches!(track.channels, 1 | 2)
            || !(8000..=96000).contains(&track.sample_rate)
        {
            return Err(MediaError::Unsupported(
                "requires AAC-LC mono/stereo at 8–96kHz",
            ));
        }
        let mut p = AudioCodecParameters::new();
        p.for_codec(CODEC_ID_AAC)
            .with_sample_rate(track.sample_rate)
            .with_channels(if track.channels == 1 {
                CHANNEL_LAYOUT_MONO
            } else {
                CHANNEL_LAYOUT_STEREO
            })
            .with_extra_data(track.extradata.clone().into_boxed_slice());
        let decoder = symphonia_codec_aac::AacDecoder::try_new(&p, &AudioDecoderOptions::default())
            .map_err(decode_err)?;
        Ok(Self {
            decoder,
            track: track.clone(),
        })
    }
    pub fn reset(&mut self) -> Result<()> {
        self.decoder.reset();
        Ok(())
    }
    pub fn decode(&mut self, sample: &Sample) -> Result<AudioFrame> {
        if sample.encryption.is_some() { return Err(MediaError::InvalidData("encrypted AAC sample requires decryption")); }
        if sample.track_id != self.track.id || sample.data.len() > MAX_SAMPLE_BYTES {
            return Err(MediaError::InvalidData("invalid AAC sample"));
        }
        let packet = PacketRef::new(
            sample.track_id,
            Timestamp::new(0),
            Duration::new(1024),
            &sample.data,
        );
        let decoded = self.decoder.decode_ref(&packet).map_err(decode_err)?;
        let mut samples = Vec::new();
        decoded.copy_to_vec_interleaved::<i16>(&mut samples);
        let spec = decoded.spec();
        let channels = spec.channels().count() as u16;
        if samples.len() > 2048
            || channels != self.track.channels
            || spec.rate() != self.track.sample_rate
        {
            return Err(MediaError::Unsupported("AAC output format changed"));
        }
        Ok(AudioFrame {
            samples,
            rate: spec.rate(),
            channels,
            pts_us: sample.pts_us,
        })
    }
}
