use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("gstreamer init failed: {0}")]
    Init(#[from] gstreamer::glib::Error),

    #[error("failed to build pipeline from description: {0}")]
    PipelineBuild(String),

    #[error("expected element {0:?} not found in pipeline")]
    ElementNotFound(&'static str),

    #[error("element {0:?} was not the expected type")]
    ElementWrongType(&'static str),

    #[error("gstreamer state change failed: {0}")]
    StateChange(#[from] gstreamer::StateChangeError),

    #[error("seek failed")]
    SeekFailed,

    #[error("no sample available (EOS or pipeline not playing)")]
    NoSample,

    #[error("sample had no buffer")]
    NoBuffer,

    #[error("sample had no caps")]
    NoCaps,

    #[error("caps could not be parsed as video info: {0}")]
    InvalidVideoInfo(String),

    #[error("failed to map buffer as readable")]
    BufferMapFailed,

    #[error("could not probe media file: {0}")]
    ProbeFailed(String),

    #[error("required GStreamer elements are missing:\n{0}")]
    MissingElements(String),
}
