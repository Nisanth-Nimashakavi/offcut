//! offcut-project: `.offcut` (RON) project file I/O.
//!
//! Deliberately a stub for Phase 1 (the roadmap: "Workspace,
//! offcut-model with full edit ops + tests, project file I/O" — the I/O
//! itself is the next slice of work after the model's edit-op tests are
//! solid, which is what this session prioritized). Wires `offcut-model`'s
//! `Project` through `serde`/`ron` end-to-end today; path relinking
//! (the product rules: "sources referenced by path... never mutates a source
//! file") is not yet implemented.

use std::path::Path;
use thiserror::Error;
use offcut_model::Project;

#[derive(Debug, Error)]
pub enum ProjectFileError {
    #[error("failed to read project file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse .offcut project file: {0}")]
    Deserialize(#[from] ron::de::SpannedError),
    #[error("failed to serialize .offcut project file: {0}")]
    Serialize(#[from] ron::Error),
}

pub fn load(path: &Path) -> Result<Project, ProjectFileError> {
    let text = std::fs::read_to_string(path)?;
    Ok(ron::from_str(&text)?)
}

pub fn save(project: &Project, path: &Path) -> Result<(), ProjectFileError> {
    let text = ron::ser::to_string_pretty(project, ron::ser::PrettyConfig::default())?;
    std::fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use offcut_model::{Rational, Source, SourceId, Time};

    #[test]
    fn round_trips_a_project_through_ron() {
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: std::path::PathBuf::from("/tmp/example.mp4"),
            duration: Time::from_nanos(60_000_000_000),
            fps: Rational::NTSC,
            resolution: (1920, 1080),
            has_audio: true,
        };
        let source_id = source.id;
        project.add_source(source);
        project.add_clip_for_source(source_id).unwrap();

        let dir = std::env::temp_dir();
        let path = dir.join(format!("offcut-test-{}.offcut", std::process::id()));
        save(&project, &path).unwrap();
        let loaded = load(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(loaded.clips.len(), 1);
        assert_eq!(loaded.sources.len(), 1);
        assert_eq!(loaded.clips[0].source, source_id);
    }
}
