//! Error type for `ggstreamlog`.

use std::io;

/// Result alias for the crate.
pub type Result<T> = std::result::Result<T, GgStreamError>;

/// Errors surfaced by the streaming log + export engine.
#[derive(Debug, thiserror::Error)]
pub enum GgStreamError {
    /// An I/O error from the durable store.
    #[error("io: {0}")]
    Io(#[from] io::Error),

    /// On-disk data was corrupt or in an unexpected format (segment header, frame, checkpoint).
    #[error("corrupt: {0}")]
    Corrupt(String),

    /// Configuration was invalid.
    #[error("config: {0}")]
    Config(String),

    /// The in-memory ingest queue is full and the backpressure policy rejects new records.
    #[error("buffer full")]
    BufferFull,

    /// The named stream does not exist.
    #[error("unknown stream: {0}")]
    UnknownStream(String),

    /// A sink/export error.
    #[error("sink: {0}")]
    Sink(String),
}
