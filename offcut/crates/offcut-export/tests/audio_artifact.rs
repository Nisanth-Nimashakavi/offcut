//! Writes real exported files into `.impeccable/` so they can be
//! inspected with an outside tool (`ffprobe`), rather than only by this
//! workspace's own assertions.
//!
//! The unit tests in `encode.rs` re-probe their output with GStreamer —
//! the same library that wrote it. That is a genuinely meaningful check
//! (it is what a GStreamer-based player does), but it shares a code base
//! with the writer, so it cannot catch a bug where we and GStreamer agree
//! on something *ffmpeg and every other demuxer disagree with*. This test
//! leaves artifacts behind precisely so a second, independent implementation
//! can be pointed at them.
//!
//! Run with:
//!   cargo test -p offcut-export --test audio_artifact -- --ignored

use std::path::{Path, PathBuf};
use offcut_export::{CancelFlag, ExportSettings, export};
use offcut_model::{Project, Source, SourceId, Speed, Time};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("repo root")
}

fn project_from(name: &str, in_secs: f64, out_secs: f64) -> Project {
    let path = repo_root().join("media").join(name);
    assert!(path.exists(), "missing fixture {}", path.display());
    let info = offcut_engine::probe_file(&path, gstreamer::ClockTime::from_seconds(15)).expect("probe");
    let mut project = Project::new();
    let source = Source {
        id: SourceId::next(),
        path,
        duration: info.duration,
        fps: info.fps,
        resolution: info.resolution,
        has_audio: info.has_audio,
    };
    let source_id = source.id;
    project.add_source(source);
    let clip = project.add_clip_for_source(source_id).expect("add clip");
    project
        .trim_clip(
            clip,
            Some(Time::from_nanos((in_secs * 1e9) as u64)),
            Some(Time::from_nanos((out_secs * 1e9) as u64)),
        )
        .expect("trim");
    project
}

#[test]
#[ignore = "writes artifacts into .impeccable/ for external inspection"]
fn write_audio_export_artifacts() {
    let out_dir = repo_root().join(".impeccable");
    std::fs::create_dir_all(&out_dir).expect("create .impeccable");
    let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };

    // 1. Plain 1x export: audio must be present AND audible.
    let plain = project_from("sample.mp4", 0.0, 3.0);
    export(&plain, &out_dir.join("audio-1x.mp4"), &settings, &CancelFlag::new(), |_| {})
        .expect("1x export");

    // 2. A 2x clip: the audio must be time-stretched, not merely cut.
    let mut fast = project_from("sample.mp4", 0.0, 4.0);
    fast.clips[0].speed = Speed::Two;
    export(&fast, &out_dir.join("audio-2x.mp4"), &settings, &CancelFlag::new(), |_| {})
        .expect("2x export");

    // 3. Muted: the track must still exist, and be digital silence.
    let mut muted = project_from("sample.mp4", 0.0, 3.0);
    muted.clips[0].muted = true;
    export(&muted, &out_dir.join("audio-muted.mp4"), &settings, &CancelFlag::new(), |_| {})
        .expect("muted export");

    for name in ["audio-1x.mp4", "audio-2x.mp4", "audio-muted.mp4"] {
        let p = out_dir.join(name);
        assert!(p.exists(), "{name} was not written");
        println!("wrote {} ({} bytes)", p.display(), std::fs::metadata(&p).unwrap().len());
    }
}
