//! offcut-app: windowless smoke-test binary (`cargo run --bin offcut-smoke`).
//!
//! The real windowed application is `main.rs` (`cargo run --bin offcut`) —
//! a genuine `iced::application` that opens a Wayland window and, as of
//! this session, actually renders live GStreamer-decoded frames on screen
//! (see the crate docs for how that was
//! confirmed). This binary predates that and is kept because it is a much
//! faster, CI-friendly check that the model/project/engine layers still
//! link and run correctly, without needing a display at all.
//!
//! What this binary does, for real: build a real `offcut-model` project,
//! then drive a real `offcut-engine::Pipeline` (a headless `videotestsrc`
//! source, since no real video file exists in this environment) through
//! play/pull/seek/stop, printing genuine frame data and position queries
//! as it goes. This is not a mock — it is the same `offcut-engine` code
//! the real unit tests exercise, run once more here as an end-to-end
//! binary-level smoke test across `offcut-model` -> `offcut-project` ->
//! `offcut-engine`. It deliberately does not touch `offcut-render` or
//! `offcut-ui` — that's `main.rs`'s job.

use gstreamer as gst;
use offcut_model::{Project, Rational, Source, SourceId, Time};

fn main() {
    // --- offcut-model + offcut-project: build and round-trip a project. ---
    let mut project = Project::new();
    let source = Source {
        id: SourceId::next(),
        path: std::path::PathBuf::from("videotestsrc://smpte"),
        duration: Time::from_nanos(1_000_000_000), // 1s, matches the engine run below
        fps: Rational::WEB_30,
        resolution: (320, 240),
        has_audio: false,
    };
    let source_id = source.id;
    project.add_source(source);
    let clip_id = project.add_clip_for_source(source_id).expect("add_clip_for_source failed");
    println!(
        "offcut-model: built a {} clip, {} source project. Total timeline duration: {:.3}s",
        project.clips.len(),
        project.sources.len(),
        project.clip(clip_id).unwrap().timeline_duration().as_secs_f64()
    );

    let tmp_path = std::env::temp_dir().join(format!("offcut-smoke-{}.offcut", std::process::id()));
    offcut_project::save(&project, &tmp_path).expect("project save failed");
    let reloaded = offcut_project::load(&tmp_path).expect("project load failed");
    std::fs::remove_file(&tmp_path).ok();
    assert_eq!(reloaded.clips.len(), project.clips.len());
    println!("offcut-project: round-tripped the project through {}", tmp_path.display());

    // --- offcut-engine: drive a real headless GStreamer pipeline. ---
    println!("\noffcut-engine: building a headless videotestsrc pipeline (30 frames @ 320x240)...");
    let pipeline = offcut_engine::Pipeline::test_pattern(320, 240, 30).expect("pipeline build failed");
    pipeline.play().expect("play failed");

    let mut frame_count = 0usize;
    let mut last_pts = Time::ZERO;
    loop {
        match pipeline.pull_frame(gst::ClockTime::from_seconds(5)) {
            Ok(frame) => {
                if frame_count == 0 {
                    println!(
                        "  first frame: {}x{} stride={} format={:?} well_formed={} non_zero={}",
                        frame.width,
                        frame.height,
                        frame.stride,
                        frame.format,
                        frame.is_well_formed(),
                        frame.has_non_zero_data()
                    );
                }
                last_pts = frame.pts;
                frame_count += 1;
            }
            Err(offcut_engine::EngineError::NoSample) => break,
            Err(e) => panic!("unexpected pipeline error: {e}"),
        }
    }
    println!("  pulled {frame_count} frames, last frame pts = {:.3}s", last_pts.as_secs_f64());
    assert_eq!(frame_count, 30, "expected exactly 30 frames from num-buffers=30");

    // Exercise the two-tier seek on a fresh pipeline —
    // the previous one already hit EOS and cannot usefully seek.
    println!("\noffcut-engine: exercising two-tier seek on a fresh pipeline...");
    let seek_pipeline = offcut_engine::Pipeline::test_pattern(320, 240, 60).expect("pipeline build failed");
    seek_pipeline.play().expect("play failed");
    let _warm = seek_pipeline.pull_frame(gst::ClockTime::from_seconds(5)).expect("warm-up pull failed");
    seek_pipeline
        .seek_fast(Time::from_nanos(500_000_000))
        .expect("fast (KEY_UNIT) seek failed");
    let after_fast_seek = seek_pipeline.pull_frame(gst::ClockTime::from_seconds(5)).expect("pull after seek failed");
    println!("  after seek_fast(0.5s): pulled frame with pts = {:.3}s", after_fast_seek.pts.as_secs_f64());
    seek_pipeline.stop().expect("stop failed");

    println!(
        "\nOK: offcut-model -> offcut-project -> offcut-engine all link and run for real in this process.\n\
         Not exercised here (this is the windowless smoke test): offcut-render's texture upload and\n\
         the on-screen display path -- see `cargo run --bin offcut` for those, and the crate docs's\n\
         \"Visual proof\" section for how they were confirmed working end to end."
    );
}
