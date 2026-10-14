#![cfg(feature = "software-media")]
use scarlet_codecs::media::*;
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixtures(PathBuf);
impl Fixtures {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::var_os("SCARLET_MEDIA_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "scarlet-media-fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn ffmpeg(&self, args: &[&str]) {
        let ffmpeg =
            std::env::var("FFMPEG").unwrap_or_else(|_| "/run/current-system/sw/bin/ffmpeg".into());
        let output = Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(args)
            .current_dir(&self.0)
            .output()
            .expect("FFMPEG must point to an ffmpeg with libx264 and AAC");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn create(&self, profile: &str, bframes: &str, fragmented: bool) -> Vec<u8> {
        let flags = if fragmented {
            "empty_moov+frag_keyframe+default_base_moof"
        } else {
            "+faststart"
        };
        self.ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=8:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=2",
            "-c:v",
            "libx264",
            "-profile:v",
            profile,
            "-pix_fmt",
            "yuv420p",
            "-bf",
            bframes,
            "-g",
            "8",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "-color_range",
            "tv",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-c:a",
            "aac",
            "-ac",
            "2",
            "-movflags",
            flags,
            "movie.mp4",
        ]);
        std::fs::read(self.0.join("movie.mp4")).unwrap()
    }
}
impl Drop for Fixtures {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn demux(data: &[u8], chunk: usize) -> (Vec<Track>, Vec<Sample>) {
    let mut parser = Mp4Parser::new();
    let mut tracks = Vec::new();
    let mut samples = Vec::new();
    let mut pull = |parser: &mut Mp4Parser| {
        while let Some(event) = parser.next_event().unwrap() {
            match event {
                ParserEvent::Track(t) => tracks.push(t),
                ParserEvent::Sample(s) => samples.push(s),
            }
        }
    };
    for part in data.chunks(chunk) {
        parser.push_bytes(part).unwrap();
        pull(&mut parser);
    }
    parser.finish().unwrap();
    pull(&mut parser);
    (tracks, samples)
}
fn decode_all(tracks: &[Track], samples: &[Sample]) -> (Vec<VideoFrame>, usize, i16) {
    let video = tracks.iter().find(|t| t.codec == Codec::H264).unwrap();
    let audio = tracks.iter().find(|t| t.codec == Codec::Aac).unwrap();
    let mut v = VideoDecoder::new(video).unwrap();
    let mut a = AudioDecoder::new(audio).unwrap();
    let mut frames = Vec::new();
    let mut audio_samples = 0;
    let mut peak = 0;
    for sample in samples {
        if sample.track_id == video.id {
            let out = v.push(sample).unwrap();
            assert!(out.len() <= 3);
            frames.extend(out);
        } else if sample.track_id == audio.id {
            let out = a.decode(sample).unwrap();
            assert_eq!((out.rate, out.channels), (48000, 2));
            audio_samples += out.samples.len();
            peak = peak.max(
                out.samples
                    .iter()
                    .map(|s| s.saturating_abs())
                    .max()
                    .unwrap_or(0),
            );
        }
    }
    let end = v.flush().unwrap();
    assert!(end.len() <= 3);
    frames.extend(end);
    assert_eq!(frames.len(), 16);
    assert!(
        frames.windows(2).all(|f| f[0].pts_us < f[1].pts_us),
        "timestamps must stay attached through reorder"
    );
    for f in &frames {
        assert_eq!((f.width, f.height), (64, 48));
        assert_eq!(f.strides, [64, 32, 32]);
        assert_eq!((f.y.len(), f.u.len(), f.v.len()), (3072, 768, 768));
        assert_eq!(
            f.color,
            ColorInfo {
                matrix: 1,
                primaries: 1,
                transfer: 1,
                full_range: false,
                chroma_location: 0
            }
        );
    }
    assert!(audio_samples > 180000);
    assert!(peak > 500);
    // A discontinuity must produce the same first decoded pixels and PTS.
    v.reset().unwrap();
    let mut restarted = Vec::new();
    for s in samples.iter().filter(|s| s.track_id == video.id) {
        restarted.extend(v.push(s).unwrap());
    }
    restarted.extend(v.flush().unwrap());
    assert_eq!(
        frames.iter().map(|f| (&f.y, f.pts_us)).collect::<Vec<_>>(),
        restarted
            .iter()
            .map(|f| (&f.y, f.pts_us))
            .collect::<Vec<_>>()
    );
    (frames, audio_samples, peak)
}
#[test]
fn baseline_progressive_mp4_actual_decode() {
    let fixtures = Fixtures::new();
    let data = fixtures.create("baseline", "0", false);
    let (tracks, samples) = demux(&data, 257);
    assert_eq!(tracks.len(), 2);
    let (frames, _, _) = decode_all(&tracks, &samples);
    fixtures.ffmpeg(&[
        "-i",
        "movie.mp4",
        "-an",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "rawvideo",
        "-fps_mode",
        "passthrough",
        "reference.yuv",
    ]);
    let reference = std::fs::read(fixtures.0.join("reference.yuv")).unwrap();
    assert_eq!(
        reference.len(),
        16 * 4608,
        "reference must preserve original presentation timestamps without CFR duplication"
    );
    let mut actual = Vec::new();
    for frame in frames {
        actual.extend(frame.y);
        actual.extend(frame.u);
        actual.extend(frame.v);
    }
    assert_eq!(
        actual, reference,
        "decoded I420 must match independent ffmpeg decoder"
    );
}
#[test]
fn main_b_frames_fragmented_incremental_decode_and_timestamps() {
    let fixtures = Fixtures::new();
    let data = fixtures.create("main", "2", true);
    let (tracks, samples) = demux(&data, 1);
    assert_eq!(tracks.len(), 2);
    let video = tracks.iter().find(|t| t.codec == Codec::H264).unwrap();
    assert!(
        samples
            .iter()
            .any(|s| s.track_id == video.id && s.pts_us != s.dts_us)
    );
    assert!(
        samples
            .iter()
            .filter(|s| s.track_id == video.id)
            .map(|s| s.dts_us)
            .max()
            .unwrap()
            > 1_000_000
    );
    let (frames, _, _) = decode_all(&tracks, &samples);
    fixtures.ffmpeg(&[
        "-i",
        "movie.mp4",
        "-an",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "rawvideo",
        "-fps_mode",
        "passthrough",
        "reference.yuv",
    ]);
    let reference = std::fs::read(fixtures.0.join("reference.yuv")).unwrap();
    assert_eq!(
        reference.len(),
        16 * 4608,
        "reference must preserve original presentation timestamps without CFR duplication"
    );
    let mut actual = Vec::new();
    for frame in frames {
        actual.extend(frame.y);
        actual.extend(frame.u);
        actual.extend(frame.v);
    }
    let differences: Vec<_> = actual
        .chunks(4608)
        .zip(reference.chunks(4608))
        .enumerate()
        .map(|(i, (a, b))| {
            (
                i,
                a.iter().zip(b).filter(|(a, b)| a != b).count(),
                a.iter()
                    .zip(b)
                    .map(|(a, b)| u64::from(a.abs_diff(*b)))
                    .sum::<u64>(),
                a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap(),
            )
        })
        .collect();
    assert!(
        actual == reference,
        "CABAC/B-frame pixel differences(frame,count,sum_abs,max): {differences:?}"
    );
}
#[test]
fn malformed_boxes_and_backpressure_are_explicit() {
    let mut p = Mp4Parser::new();
    assert_eq!(
        p.push_bytes(&vec![0; MAX_BUFFER_BYTES + 1]),
        Err(MediaError::BufferFull)
    );
    p.push_bytes(&[0, 0, 0, 7, b'f', b't', b'y', b'p']).unwrap();
    assert!(matches!(p.next_event(), Err(MediaError::InvalidData(_))));
    p.reset();
    p.push_bytes(&[0, 0, 0, 16, b'f', b'r', b'e', b'e', 0])
        .unwrap();
    p.finish().unwrap();
    assert!(matches!(p.next_event(), Err(MediaError::InvalidData(_))));
    p.reset();
    p.push_bytes(&[0, 0, 0, 8, b'f', b'r', b'e', b'e']).unwrap();
    assert!(p.next_event().unwrap().is_none());
}

fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(payload);
    out
}
#[test]
fn fragment_abort_retains_initialization_and_large_free_padding_is_legal() {
    let fixtures = Fixtures::new();
    let data = fixtures.create("baseline", "0", true);
    let mut cursor = 0;
    let mut init_end = 0;
    while cursor + 8 <= data.len() {
        let size = u32::from_be_bytes(data[cursor..cursor + 4].try_into().unwrap()) as usize;
        if &data[cursor + 4..cursor + 8] == b"moov" {
            init_end = cursor + size;
            break;
        }
        cursor += size;
    }
    assert!(init_end > 0);
    let mut init = boxed(b"free", &vec![0; 2 * 1024 * 1024]);
    init.extend_from_slice(&data[..init_end]);
    let mut parser = Mp4Parser::new();
    let mut tracks = Vec::new();
    for bytes in init.chunks(512 * 1024) {
        parser.push_bytes(bytes).unwrap();
        while let Some(event) = parser.next_event().unwrap() {
            if let ParserEvent::Track(t) = event {
                tracks.push(t);
            } else {
                panic!("sample before media");
            }
        }
    }
    assert_eq!(tracks.len(), 2);
    parser.push_bytes(&data[init_end..init_end + 3]).unwrap();
    assert!(parser.next_event().unwrap().is_none());
    parser.reset();
    let mut samples = Vec::new();
    for bytes in data[init_end..].chunks(257) {
        parser.push_bytes(bytes).unwrap();
        while let Some(event) = parser.next_event().unwrap() {
            match event {
                ParserEvent::Track(_) => panic!("reset lost init configuration"),
                ParserEvent::Sample(s) => samples.push(s),
            }
        }
    }
    parser.finish().unwrap();
    assert!(parser.next_event().unwrap().is_none());
    decode_all(&tracks, &samples);
}
#[test]
fn hostile_sample_size_is_rejected_before_demux_allocation() {
    let mut stsz = vec![0; 4];
    stsz.extend(u32::MAX.to_be_bytes());
    stsz.extend(1u32.to_be_bytes());
    let bytes = boxed(b"moov", &boxed(b"stsz", &stsz));
    let mut parser = Mp4Parser::new();
    parser.push_bytes(&bytes).unwrap();
    assert!(matches!(
        parser.next_event(),
        Err(MediaError::Unsupported("oversized encoded sample"))
    ));
}
#[test]
fn inband_sps_cannot_allocate_oversized_pictures() {
    let fixtures = Fixtures::new();
    let data = fixtures.create("baseline", "0", false);
    let (tracks, _) = demux(&data, 257);
    let track = tracks.iter().find(|t| t.codec == Codec::H264).unwrap();
    fixtures.ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=red:size=4096x16:duration=0.1:rate=1",
        "-c:v",
        "libx264",
        "-profile:v",
        "baseline",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "h264",
        "large.h264",
    ]);
    let annex = std::fs::read(fixtures.0.join("large.h264")).unwrap();
    // Convert the original Annex-B NAL bytes (including emulation prevention)
    // to AVCC, without asking a decoder to allocate the oversized picture.
    let starts: Vec<_> = annex
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == [0, 0, 0, 1])
        .map(|(i, _)| i)
        .collect();
    let start = *starts.iter().find(|&&i| annex[i + 4] & 31 == 7).unwrap();
    let end = starts
        .iter()
        .copied()
        .find(|&i| i > start)
        .unwrap_or(annex.len());
    let nal = &annex[start + 4..end];
    let mut avcc = (nal.len() as u32).to_be_bytes().to_vec();
    avcc.extend(nal);
    let mut decoder = VideoDecoder::new(track).unwrap();
    let sample = Sample {
        track_id: track.id,
        data: avcc,
        pts_us: 0,
        dts_us: 0,
        duration_us: 1,
        keyframe: true,
    };
    assert!(matches!(
        decoder.push(&sample),
        Err(MediaError::Unsupported(_))
    ));
}

fn compare_b_frame_reference(fixtures: &Fixtures, data: &[u8], profile: u8) {
    let (tracks, samples) = demux(data, 257);
    let video = tracks.iter().find(|t| t.codec == Codec::H264).unwrap();
    assert_eq!(
        video.extradata[1], profile,
        "fixture must encode the requested AVC profile"
    );
    let (frames, _, _) = decode_all(&tracks, &samples);
    let mut expected_pts = samples
        .iter()
        .filter(|s| s.track_id == video.id)
        .map(|s| s.pts_us)
        .collect::<Vec<_>>();
    assert_eq!(expected_pts.len(), 16);
    assert!(
        samples
            .iter()
            .any(|s| s.track_id == video.id && s.pts_us != s.dts_us),
        "fixture must contain reordered pictures"
    );
    expected_pts.sort_unstable();
    assert_eq!(
        frames.iter().map(|f| f.pts_us).collect::<Vec<_>>(),
        expected_pts,
        "decoded pixels must retain their source packet PTS through reorder and IDR boundaries"
    );
    assert!(expected_pts.windows(2).all(|p| p[1] - p[0] == 125_000));
    fixtures.ffmpeg(&[
        "-i",
        "movie.mp4",
        "-an",
        "-fps_mode",
        "passthrough",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "rawvideo",
        "reference.yuv",
    ]);
    let reference = std::fs::read(fixtures.0.join("reference.yuv")).unwrap();
    assert_eq!(
        reference.len(),
        16 * 4608,
        "independent decoder must preserve all 16 source frames without CFR insertion"
    );
    for (i, (frame, expected)) in frames.iter().zip(reference.chunks_exact(4608)).enumerate() {
        let actual = frame
            .y
            .iter()
            .chain(&frame.u)
            .chain(&frame.v)
            .copied()
            .collect::<Vec<_>>();
        assert!(
            actual == expected,
            "AVC profile {profile} frame {i}: {} I420 byte differences, max error {}",
            actual.iter().zip(expected).filter(|(a, b)| a != b).count(),
            actual
                .iter()
                .zip(expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap()
        );
    }
}

#[test]
fn high_b_frames_fragmented_pixels_and_pts_match_independent_decoder() {
    let fixtures = Fixtures::new();
    let data = fixtures.create("high", "2", true);
    compare_b_frame_reference(&fixtures, &data, 100);
}

#[test]
fn main_and_high_nonidentity_weighted_prediction_match_independent_decoder() {
    for (profile_name, profile_id) in [("main", 77), ("high", 100)] {
        let fixtures = Fixtures::new();
        fixtures.ffmpeg(&[
            "-f", "lavfi", "-i", "testsrc2=size=64x48:rate=8:duration=2,fade=t=out:st=0:d=2",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2",
            "-c:v", "libx264", "-profile:v", profile_name, "-pix_fmt", "yuv420p", "-bf", "2", "-g", "8",
            "-x264-params", "colorprim=bt709:transfer=bt709:colormatrix=bt709:weightp=2:weightb=1:b-adapt=0:b-pyramid=normal:ref=3",
            "-color_range", "tv", "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709",
            "-c:a", "aac", "-ac", "2", "-movflags", "empty_moov+frag_keyframe+default_base_moof", "movie.mp4",
        ]);
        // Verify the encoded stream actually contains nonidentity explicit P
        // weights and enables implicit B weighting; encoder settings alone do
        // not guarantee that weighted prediction is exercised.
        let ffmpeg =
            std::env::var("FFMPEG").unwrap_or_else(|_| "/run/current-system/sw/bin/ffmpeg".into());
        let trace = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "info",
                "-i",
                "movie.mp4",
                "-map",
                "0:v:0",
                "-c:v",
                "copy",
                "-bsf:v",
                "trace_headers",
                "-f",
                "null",
                "-",
            ])
            .current_dir(&fixtures.0)
            .output()
            .unwrap();
        assert!(
            trace.status.success(),
            "trace_headers must inspect the generated stream"
        );
        let trace = String::from_utf8_lossy(&trace.stderr);
        let mut denom = 0;
        let mut nonidentity = false;
        let mut implicit_b = false;
        for line in trace.lines() {
            let Some((field, value)) = line.rsplit_once('=') else {
                continue;
            };
            let Ok(value) = value.trim().parse::<i32>() else {
                continue;
            };
            if field.contains("luma_log2_weight_denom") {
                denom = value;
            }
            if field.contains("luma_weight_l0[") && value != 1 << denom {
                nonidentity = true;
            }
            if field.contains("luma_offset_l0[") && value != 0 {
                nonidentity = true;
            }
            if field.contains("weighted_bipred_idc") && value == 2 {
                implicit_b = true;
            }
        }
        assert!(
            nonidentity && implicit_b,
            "fixture must encode active P weights and implicit B weighting"
        );
        let data = std::fs::read(fixtures.0.join("movie.mp4")).unwrap();
        compare_b_frame_reference(&fixtures, &data, profile_id);
    }
}
