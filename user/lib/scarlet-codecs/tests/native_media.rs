#![cfg(feature = "media-container-aac")]
use scarlet_codecs::media::*;
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::var_os("SCARLET_MEDIA_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = root.join(format!(
            "scarlet-native-avc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn ffmpeg(&self, args: &[&str]) {
        let tool =
            std::env::var("FFMPEG").unwrap_or_else(|_| "/run/current-system/sw/bin/ffmpeg".into());
        let output = Command::new(tool)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn create(&self, profile: &str, bf: &str) -> Vec<u8> {
        self.ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=8:duration=2",
            "-c:v",
            "libx264",
            "-profile:v",
            profile,
            "-bf",
            bf,
            "-g",
            "8",
            "-x264-params",
            "colorprim=bt709:transfer=bt709:colormatrix=bt709",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "empty_moov+frag_keyframe+default_base_moof",
            "movie.mp4",
        ]);
        std::fs::read(self.0.join("movie.mp4")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn parsed(data: &[u8]) -> (Track, Vec<Sample>) {
    let mut parser = Mp4Parser::new();
    let mut track = None;
    let mut samples = Vec::new();
    for bytes in data.chunks(257) {
        parser.push_bytes(bytes).unwrap();
        while let Some(event) = parser.next_event().unwrap() {
            match event {
                ParserEvent::Track(t) => track = Some(t),
                ParserEvent::Sample(s) => samples.push(s),
            }
        }
    }
    parser.finish().unwrap();
    while let Some(event) = parser.next_event().unwrap() {
        if let ParserEvent::Sample(s) = event {
            samples.push(s);
        }
    }
    (track.unwrap(), samples)
}
#[test]
fn native_avcc_transport_preserves_ebsp_pixels_colour_and_reorder_metadata() {
    for (profile, bf, depth) in [("baseline", "0", 0), ("main", "2", 2), ("high", "2", 2)] {
        let fixture = Fixture::new();
        let bytes = fixture.create(profile, bf);
        let (track, samples) = parsed(&bytes);
        let mut config = AvcConfiguration::new(&track).unwrap();
        assert_eq!((config.width(), config.height()), (64, 48));
        assert_eq!(config.max_reorder_frames(), depth);
        assert_eq!(
            config.color(),
            ColorInfo {
                matrix: 1,
                primaries: 1,
                transfer: 1,
                full_range: false,
                chroma_location: 0
            }
        );
        let mut annex = Vec::new();
        for sample in &samples {
            annex.extend(config.annex_b(sample).unwrap());
        }
        std::fs::write(fixture.0.join("access-units.h264"), annex).unwrap();
        for (input, output) in [
            ("movie.mp4", "reference.yuv"),
            ("access-units.h264", "transport.yuv"),
        ] {
            fixture.ffmpeg(&[
                "-i",
                input,
                "-pix_fmt",
                "yuv420p",
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
                output,
            ]);
        }
        let expected = std::fs::read(fixture.0.join("reference.yuv")).unwrap();
        let actual = std::fs::read(fixture.0.join("transport.yuv")).unwrap();
        assert_eq!(expected.len(), 16 * 4608);
        assert!(
            actual == expected,
            "Annex-B framing must preserve native decoder input pixels"
        );
    }
}
#[test]
fn native_config_rejects_truncated_and_oversized_inband_sps_before_submit() {
    let fixture = Fixture::new();
    let bytes = fixture.create("baseline", "0");
    let (mut track, samples) = parsed(&bytes);
    let mut config = AvcConfiguration::new(&track).unwrap();
    let mut empty = samples[0].clone();
    empty.data.clear();
    assert!(matches!(
        config.annex_b(&empty),
        Err(MediaError::InvalidData(
            "AVC access unit requires one coded picture"
        ))
    ));
    let mut truncated = samples[0].clone();
    truncated.data = vec![0, 0, 0, 8, 0x67];
    assert!(config.annex_b(&truncated).is_err());
    fixture.ffmpeg(&[
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
    let bytes = std::fs::read(fixture.0.join("large.h264")).unwrap();
    let starts: Vec<_> = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == [0, 0, 0, 1])
        .map(|(i, _)| i)
        .collect();
    let start = *starts.iter().find(|&&i| bytes[i + 4] & 31 == 7).unwrap();
    let end = starts
        .iter()
        .copied()
        .find(|&i| i > start)
        .unwrap_or(bytes.len());
    let nal = &bytes[start + 4..end];
    let mut oversized = samples[0].clone();
    oversized.data = (nal.len() as u32).to_be_bytes().to_vec();
    oversized.data.extend(nal);
    assert!(matches!(
        config.annex_b(&oversized),
        Err(MediaError::Unsupported(_))
    ));
    assert_eq!((config.width(), config.height()), (64, 48)); // rejected update is transactional
    track.extradata.truncate(6);
    assert!(AvcConfiguration::new(&track).is_err());
}

#[test]
fn multiple_sps_configuration_is_explicitly_unsupported() {
    let fixture = Fixture::new();
    let bytes = fixture.create("baseline", "0");
    let (mut track, _) = parsed(&bytes);
    let length = usize::from(u16::from_be_bytes(
        track.extradata[6..8].try_into().unwrap(),
    ));
    let sps = track.extradata[6..8 + length].to_vec();
    track.extradata[5] = (track.extradata[5] & 0xe0) | 2;
    track.extradata.splice(8 + length..8 + length, sps);
    assert!(matches!(
        AvcConfiguration::new(&track),
        Err(MediaError::Unsupported("multiple AVC SPS configurations"))
    ));
}
