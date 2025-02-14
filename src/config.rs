#[derive(Copy, Clone, Debug)]
/// Config struct for `WebSocketMultiplexor<T>`.
pub struct Config {
    /// Frames larger than this size will be dropped.
    pub max_frame_size: usize,
    /// Buffer size when reading / writing to vended streams,
    /// should be at least 512 bytes smaller than `max_frame_size`.
    pub buf_size: usize,
    /// How many frames can we queue in our inner channel before
    /// we block.
    pub max_queued_frames: usize,
    /// How many pending connections do we queue waiting on
    /// `accept()` to be called.
    pub accept_queue_len: usize,
    /// An identifier for this `WebSocketMultiplexor<T>`.
    /// Used in tracing logs.
    pub identifier: &'static str,
}

impl Default for Config {
    /// Construct a default `Config`
    fn default() -> Self {
        Self {
            max_frame_size: 4 * 1024 * 1024,
            buf_size: 1024 * 1024,
            max_queued_frames: 256,
            accept_queue_len: 16,
            identifier: "",
        }
    }
}

impl Config {
    /// Add identifier static &str to config
    #[must_use]
    pub fn with_identifier(mut self, identifier: &'static str) -> Self {
        self.identifier = identifier;
        self
    }
}
