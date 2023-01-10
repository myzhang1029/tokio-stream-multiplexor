use futures_util::StreamExt;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::WebSocketStream;
use tungstenite::protocol::Role;
use websocket_multiplexor::{Config, WebSocketMultiplexor};

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let (stream_0, stream_1) = duplex(10);

    let ws_0 = WebSocketStream::from_raw_socket(stream_0, Role::Client, None).await;
    let (sink_0, stream_0) = ws_0.split();
    let ws_1 = WebSocketStream::from_raw_socket(stream_1, Role::Server, None).await;
    let (sink_1, stream_1) = ws_1.split();

    let mux_0 = WebSocketMultiplexor::new(sink_0, stream_0, Config::default());
    let mux_1 = WebSocketMultiplexor::new(sink_1, stream_1, Config::default());

    let listener = mux_0.bind(23).await?;
    tokio::spawn(async move {
        while let Ok(mut stream) = listener.accept().await {
            let _ = stream.write_all(b"Hello, world!").await;
        }
    });

    let mut stream = mux_1.connect(23).await?;
    let mut buf = [0u8; 16];
    if let Ok(bytes) = stream.read(&mut buf).await {
        println!("{:?}", std::str::from_utf8(&buf[0..bytes]));
    }

    Ok(())
}
