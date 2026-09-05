//! offcut-ui: widgets and the Shell view (the crate layout,
//! Phase 2). `theme` holds the Dark/Light token table transcribed from
//! The design system; `shell` builds the actual window layout — titlebar,
//! stage, inspector, transport, timeline — from real `iced` widgets
//! driven by a real `offcut_model::Project`.

pub mod icons;
pub mod rice;
pub mod shell;
pub mod timeline;
pub mod theme;
pub mod trimbar;
pub mod video;

pub use icons::{Icon, icon};
pub use shell::{AdjustField, ExportState, InspectorTab, ShellMessage, ShellState, view};
pub use timeline::TimelineMessage;
pub use trimbar::{TrimBarData, TrimBarMessage};
pub use theme::{Mode, Palette};
pub use video::{VideoWidget, video_preview};
