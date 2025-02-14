//! # Tokio Stream Multiplexor
//!
//! TL;DR: Multiplex multiple streams over a single stream. Has a `TcpListener` / `TcpSocket` style interface, and uses u16 ports similar to TCP itself.
//!
//! ```toml
//! [dependencies]
//! foo = "1.2.3"
//! ```
//!
//! ## Why?
//!
//! Because sometimes you wanna cram as much shite down one TCP stream as you possibly can, rather than have your application connect with multiple ports.
//!
//! ## But Whyyyyyy?
//!
//! Because I needed it. Inspired by [async-smux](https://github.com/black-binary/async-smux), written for [Tokio](https://tokio.rs/).
//!
//! ## What about performance?
//! Doesn't this whole protocol in a protocol thing hurt perf?
//!
//! Sure, but take a look at the benches:
//!
//! ```ignore
//! throughput/tcp          time:   [28.968 ms 30.460 ms 31.744 ms]
//!                         thrpt:  [7.8755 GiB/s 8.2076 GiB/s 8.6303 GiB/s]
//!
//! throughput/mux          time:   [122.24 ms 135.96 ms 158.80 ms]
//!                         thrpt:  [1.5743 GiB/s 1.8388 GiB/s 2.0451 GiB/s]
//! ```
//!
//! Approximately 4.5 times slower than TCP, but still able to shovel 1.8 GiB/s of shite... Seems alright to me. (Numbers could possibly be improved with some tuning of the config params too.)

#![warn(missing_docs)]

mod config;
mod frame;
mod inner;
mod listener;
mod socket;

use std::{
    collections::HashMap,
    fmt::{Debug, Formatter, Result as FmtResult},
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

extern crate async_channel;
use futures_util::{Sink as FutureSink, Stream as FutureStream};
use rand::Rng;
pub use tokio::io::DuplexStream;
use tokio::sync::{mpsc, watch, RwLock};
use tracing::{debug, trace};
use tungstenite::Message;

pub use config::Config;
use inner::WebSocketMultiplexorInner;
pub use listener::MuxListener;
use socket::MuxSocket;

/// Result type returned by `bind()`, `accept()`, and `connect()`.
pub type Result<T> = std::result::Result<T, io::Error>;

/// The Stream Multiplexor.
///
/// # Drop
/// When the `WebSocketMultiplexor` is dropped, it will send RST to all open
/// connections and listeners.
#[derive(Clone)]
pub struct WebSocketMultiplexor<Sink, Stream> {
    inner: Arc<WebSocketMultiplexorInner<Sink, Stream>>,
}

impl<Sink, Stream> Debug for WebSocketMultiplexor<Sink, Stream> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("WebSocketMultiplexor")
            .field("id", &self.inner.config.identifier)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<Sink, Stream> Drop for WebSocketMultiplexor<Sink, Stream> {
    fn drop(&mut self) {
        self.close();
        debug!("drop {:?}", self);
    }
}

impl<Sink, Stream> WebSocketMultiplexor<Sink, Stream> {
    /// Start processing the inner stream.
    ///
    /// Only effective on a paused `WebSocketMultiplexor<T>`.
    /// See `new_paused(inner: T, config: Config)`.
    pub fn start(&self) {
        self.inner.running.send_replace(true);
    }

    /// Shut down the `WebSocketMultiplexor<T>` instance and drop reference
    /// to the inner stream to close it.
    pub fn close(&self) {
        self.inner.watch_connected_send.send_replace(false);
    }
}

impl<Sink, Stream> WebSocketMultiplexor<Sink, Stream>
where
    Sink: FutureSink<Message, Error = tungstenite::Error> + Send + Unpin + 'static,
    Stream: FutureStream<Item = tungstenite::Result<Message>> + Send + Unpin + 'static,
{
    /// Constructs a new `WebSocketMultiplexor<T>`.
    pub fn new(sink: Sink, stream: Stream, config: Config) -> Self {
        Self::new_running(sink, stream, config, true)
    }

    /// Constructs a new paused `WebSocketMultiplexor<T>`.
    ///
    /// This allows you to bind and listen on a bunch of ports before
    /// processing any packets from the inner stream, removing race conditions
    /// between bind and connect. Call `start()` to start processing the
    /// inner stream.
    pub fn new_paused(sink: Sink, stream: Stream, config: Config) -> Self {
        Self::new_running(sink, stream, config, false)
    }

    fn new_running(sink: Sink, stream: Stream, config: Config, running: bool) -> Self {
        let (send, recv) = mpsc::channel(config.max_queued_frames);
        let (watch_connected_send, watch_connected_recv) = watch::channel(true);
        let (running, _) = watch::channel(running);
        let (may_close_listeners_send, may_close_listeners_recv) = mpsc::unbounded_channel();
        let (may_close_connections_send, may_close_connections_recv) = mpsc::unbounded_channel();
        let inner = Arc::from(WebSocketMultiplexorInner {
            config,
            connected: AtomicBool::from(true),
            port_connections: RwLock::from(HashMap::new()),
            port_listeners: RwLock::from(HashMap::new()),
            watch_connected_send,
            may_close_listeners: may_close_listeners_send,
            may_close_connections: may_close_connections_send,
            send: RwLock::from(send),
            running,
        });

        tokio::spawn(inner.clone().frame_writer_sender(recv, sink));
        tokio::spawn(inner.clone().frame_reader_sender(stream));
        tokio::spawn(inner.clone().handle_mux_state_change(
            watch_connected_recv,
            may_close_listeners_recv,
            may_close_connections_recv,
        ));

        Self { inner }
    }

    /// Bind to port and return a `MuxListener<T>`.
    #[tracing::instrument]
    pub async fn bind(&self, port: u16) -> Result<MuxListener<Sink, Stream>> {
        trace!("");
        if !self.inner.connected.load(Ordering::Relaxed) {
            return Err(io::Error::from(io::ErrorKind::ConnectionReset));
        }
        let mut port = port;
        if port == 0 {
            while port < 1024 || self.inner.port_listeners.read().await.contains_key(&port) {
                port = rand::thread_rng().gen_range(1024u16..u16::MAX);
            }
            trace!("port = {}", port);
        } else if self.inner.port_listeners.read().await.contains_key(&port) {
            trace!("port_listeners already contains {}", port);
            return Err(io::Error::from(io::ErrorKind::AddrInUse));
        }
        let (send, recv) = async_channel::bounded(self.inner.config.accept_queue_len);
        self.inner.port_listeners.write().await.insert(port, send);
        Ok(MuxListener::new(self.inner.clone(), port, recv))
    }

    /// Connect to `port` on the remote end.
    #[tracing::instrument]
    pub async fn connect(&self, port: u16) -> Result<DuplexStream> {
        trace!("");
        if !self.inner.connected.load(Ordering::Relaxed) {
            trace!("Not connected, raise Error");
            return Err(io::Error::from(io::ErrorKind::ConnectionReset));
        }
        let mut sport: u16 = 0;
        while sport < 1024
            || self
                .inner
                .port_connections
                .read()
                .await
                .contains_key(&(sport, port))
        {
            sport = rand::thread_rng().gen_range(1024u16..u16::MAX);
        }
        trace!("sport = {}", sport);

        let mux_socket = MuxSocket::new(self.inner.clone(), sport, port, false);
        let mut rx = mux_socket.stream().await;
        self.inner
            .port_connections
            .write()
            .await
            .insert((sport, port), mux_socket.clone());
        mux_socket.start().await;

        rx.recv()
            .await
            .ok_or_else(|| io::Error::from(io::ErrorKind::Other))?
    }

    /// Return a `tokio::sync::watch::Receiver` that will update to `false`
    /// when the inner stream closes.
    #[tracing::instrument]
    pub fn watch_connected(&self) -> watch::Receiver<bool> {
        trace!("");
        self.inner.watch_connected_send.subscribe()
    }
}

#[cfg(test)]
mod test;
