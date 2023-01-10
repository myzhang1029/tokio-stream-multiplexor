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

use futures_util::sink::SinkExt;
use futures_util::stream::StreamExt;
use futures_util::{Sink as FutureSink, Stream as FutureStream};
use tokio::{
    io::DuplexStream,
    sync::{mpsc, watch, RwLock},
};
use tracing::{debug, error, trace};
use tungstenite::Message;

use crate::{
    config::Config,
    frame::{Flag, Frame},
    socket::MuxSocket,
};

type PortPair = (u16, u16);

pub(crate) struct WebSocketMultiplexorInner<Sink, Stream> {
    pub config: Config,
    pub connected: AtomicBool,
    pub port_connections: RwLock<HashMap<PortPair, Arc<MuxSocket<Sink, Stream>>>>,
    pub port_listeners: RwLock<HashMap<u16, async_channel::Sender<DuplexStream>>>,
    /// The sender for the watch channel that is used to signal that the mux is connected or not.
    pub watch_connected_send: watch::Sender<bool>,
    /// The sender of ports that may be freed.
    pub may_close_listeners: mpsc::UnboundedSender<u16>,
    /// The sender of connection ports that may be freed.
    pub may_close_connections: mpsc::UnboundedSender<PortPair>,
    pub send: RwLock<mpsc::Sender<Frame>>,
    /// The sender for the watch channel that is used to signal that the mux is running or not.
    pub running: watch::Sender<bool>,
}

impl<Sink, Stream> Debug for WebSocketMultiplexorInner<Sink, Stream> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct("WebSocketMultiplexorInner")
            .field("id", &self.config.identifier)
            .field("connected", &self.connected)
            .finish()
    }
}

impl<Sink, Stream> Drop for WebSocketMultiplexorInner<Sink, Stream> {
    fn drop(&mut self) {
        self.watch_connected_send.send_replace(false);
        debug!("drop {:?}", self);
    }
}

impl<Sink, Stream> WebSocketMultiplexorInner<Sink, Stream>
where
    Sink: FutureSink<Message, Error = tungstenite::Error> + Unpin + 'static,
    Stream: FutureStream<Item = tungstenite::Result<Message>> + Unpin + 'static,
{
    #[tracing::instrument(skip(recv, frame_sink), level = "trace")]
    pub async fn frame_writer_sender(
        self: Arc<Self>,
        mut recv: mpsc::Receiver<Frame>,
        mut frame_sink: Sink,
    ) {
        let mut running = self.running.subscribe();
        let mut connected = self.watch_connected_send.subscribe();
        while !*running.borrow() {
            if let Err(error) = running.changed().await {
                error!("Error {:?} receiving running state", error);
            }
        }

        loop {
            if !*connected.borrow() {
                trace!("Running false");
                break;
            }
            let frame = tokio::select! {
                res = recv.recv() => {
                    if let Some(value) = res {
                        value
                    } else {
                        error!("Error {:?} reading from stream", res);
                        self.watch_connected_send.send_replace(false);
                        break;
                    }
                }
                _ = connected.changed() => {
                    trace!("Connected changed");
                    continue;
                }
            };
            match frame.try_into() {
                Ok(message) => {
                    if let Err(error) = frame_sink.send(message).await {
                        error!("Error {:?} sending to stream", error);
                        self.watch_connected_send.send_replace(false);
                        break;
                    }
                }
                Err(error) => {
                    error!("Error {:?} converting frame to message", error);
                    self.watch_connected_send.send_replace(false);
                    break;
                }
            }
        }
    }

    #[tracing::instrument(skip(frame_stream), level = "trace")]
    pub async fn frame_reader_sender(self: Arc<Self>, mut frame_stream: Stream) {
        let mut running = self.running.subscribe();
        let mut connected = self.watch_connected_send.subscribe();
        while !*running.borrow() {
            if let Err(error) = running.changed().await {
                error!("Error {:?} receiving running state", error);
            }
        }

        loop {
            if !*connected.borrow() {
                trace!("Running false");
                break;
            }
            let frame: Frame = tokio::select! {
                res = frame_stream.next() => {
                    if let Some(Ok(message)) = res {
                        match Frame::try_from(message) {
                            Ok(frame) => frame,
                            Err(error) => {
                                error!("Error {:?} converting message to frame", error);
                                self.watch_connected_send.send_replace(false);
                                break;
                            }
                        }
                    } else {
                        error!("Error {:?} reading from framed_reader", res);
                        self.watch_connected_send.send_replace(false);
                        break;
                    }
                }
                _ = connected.changed() => {
                    trace!("Connected changed");
                    continue;
                }
            };
            if matches!(frame.flag, Flag::Syn)
                && self.port_listeners.read().await.contains_key(&frame.dport)
            {
                trace!("Syn received for listener, vending MuxSocket");
                let socket = MuxSocket::new(self.clone(), frame.dport, frame.sport, true);
                self.port_connections
                    .write()
                    .await
                    .insert((frame.dport, frame.sport), socket.clone());
                socket.recv_frame(frame).await;
            } else if let Some(socket) = self
                .port_connections
                .read()
                .await
                .get(&(frame.dport, frame.sport))
            {
                trace!("Frame received for active socket {:?}", socket);
                socket.recv_frame(frame).await;
            } else if !matches!(frame.flag, Flag::Rst) {
                trace!(
                    "Frame received for unknown (dport, sport) ({}, {}), sending Rst",
                    frame.dport,
                    frame.sport
                );
                let framed_writer = self.send.write().await;
                if let Err(error) = framed_writer
                    .send(Frame::new_reply(&frame, Flag::Rst, 0))
                    .await
                {
                    error!("Error {:?} sending Rst", error);
                }
            }
        }
    }

    /// Process `may_close_listeners_recv` channel.
    /// Use in a `select!` statement.
    async fn process_may_close_listeners_once(
        &self,
        may_close_listeners_recv: &mut mpsc::UnboundedReceiver<u16>,
    ) {
        if let Some(dport) = may_close_listeners_recv.recv().await {
            debug!("Freeing listener at port {}", dport);
            let mut port_listeners = self.port_listeners.write().await;
            if let Some(listener) = port_listeners.remove(&dport) {
                listener.close();
            }
        }
    }

    /// Process `may_close_connections_recv` channel.
    /// Use in a `select!` statement.
    async fn process_may_close_connections_once(
        &self,
        may_close_connections_recv: &mut mpsc::UnboundedReceiver<PortPair>,
    ) {
        if let Some((dport, sport)) = may_close_connections_recv.recv().await {
            debug!("Freeing connection from port {} to port {}", sport, dport);
            let mut port_connections = self.port_connections.write().await;
            port_connections.remove(&(dport, sport));
        }
    }

    /// Mux maintenance task.
    /// - Free ports when the listener is dropped.
    /// - Free ports when the socket is dropped.
    /// - RST all connections when the mux is disconnected/dropped.
    #[tracing::instrument(level = "debug")]
    pub async fn handle_mux_state_change(
        self: Arc<Self>,
        mut watch_connected_recv: watch::Receiver<bool>,
        mut may_close_listeners_recv: mpsc::UnboundedReceiver<u16>,
        mut may_close_connections_recv: mpsc::UnboundedReceiver<(u16, u16)>,
    ) {
        if *watch_connected_recv.borrow() {
            loop {
                tokio::select! {
                    r = watch_connected_recv.changed() => {
                        if !*watch_connected_recv.borrow() || r.is_err() {
                            break;
                        }
                    }
                    _ = self.process_may_close_connections_once(&mut may_close_connections_recv) => {}
                    _ = self.process_may_close_listeners_once(&mut may_close_listeners_recv) => {}
                }
            }
        }

        self.connected.store(false, Ordering::Relaxed);

        for (_, connection) in self.port_connections.write().await.drain() {
            trace!("Send rst to {:?}", connection);
            if connection.rst.send(true).is_err() {
                error!("Error sending rst to connection {:?}", connection);
            }
            if let Some(sender) = connection.external_stream_sender.write().await.as_ref() {
                trace!("Send Error to {:?} external_stream_reader", connection);
                if let Err(error) = sender
                    .send(Err(io::Error::from(io::ErrorKind::BrokenPipe)))
                    .await
                {
                    error!("Error {:?} dropping port_connections", error);
                }
            }
        }
        self.port_listeners.write().await.clear();
    }
}
