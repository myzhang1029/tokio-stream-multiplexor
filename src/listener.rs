use std::{
    fmt::{Debug, Formatter, Result as FmtResult},
    io,
    sync::Arc,
};

extern crate async_channel;
pub use tokio::io::DuplexStream;
use tracing::{debug, trace};

use crate::{inner::WebSocketMultiplexorInner, Result};

/// Listener struct returned by `WebSocketMultiplexor<T>::bind()`
///
/// # Drop
/// When the listener is dropped, it will free the port for reuse, but established
/// connections will not be closed.
pub struct MuxListener<Sink, Stream> {
    inner: Arc<WebSocketMultiplexorInner<Sink, Stream>>,
    port: u16,
    recv: async_channel::Receiver<DuplexStream>,
}

impl<Sink, Stream> MuxListener<Sink, Stream> {
    pub(crate) fn new(
        inner: Arc<WebSocketMultiplexorInner<Sink, Stream>>,
        port: u16,
        recv: async_channel::Receiver<DuplexStream>,
    ) -> Self {
        Self { inner, port, recv }
    }
}

impl<Sink, Stream> Debug for MuxListener<Sink, Stream> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("MuxListener")
            .field("id", &self.inner.config.identifier)
            .field("port", &self.port)
            .finish()
    }
}

impl<Sink, Stream> Drop for MuxListener<Sink, Stream> {
    fn drop(&mut self) {
        self.inner.may_close_listeners.send(self.port).ok();
        debug!("drop {:?}", self);
    }
}

impl<Sink, Stream> MuxListener<Sink, Stream> {
    /// Accept a connection from the remote side
    #[tracing::instrument(level = "debug")]
    pub async fn accept(&self) -> Result<DuplexStream> {
        trace!("");
        self.recv
            .recv()
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    /// Get the port number of this listener
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }
}
