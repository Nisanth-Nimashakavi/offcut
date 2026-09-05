use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("the project has no clips to export")]
    EmptyTimeline,

    #[error("this machine is missing GStreamer elements needed to export:\n{0}")]
    MissingElements(String),

    #[error("engine error during export: {0}")]
    Engine(#[from] offcut_engine::EngineError),

    #[error("gpu error while baking crop/adjust: {0}")]
    Render(String),

    #[error("failed to build the encode pipeline: {0}")]
    PipelineBuild(String),

    #[error("the encoder reported an error: {0}")]
    Encode(String),

    #[error("could not write to {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("export was cancelled")]
    Cancelled,
}
