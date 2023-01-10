use bytes::Buf;
use tracing::warn;
use tungstenite::Message;

#[derive(Debug)]
pub enum Flag {
    Syn = 0,
    SynAck = 1,
    Ack = 2,
    Rst = 3,
    Fin = 4,
    Unset = 5,
}

pub struct Frame {
    pub sport: u16,
    pub dport: u16,
    pub flag: Flag,
    pub seq: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("sport", &self.sport)
            .field("dport", &self.dport)
            .field("flag", &self.flag)
            .field("seq", &self.seq)
            .field("data.len", &self.data.len())
            .finish()
    }
}

impl Frame {
    pub fn new_no_data(sport: u16, dport: u16, flag: Flag, seq: u32) -> Self {
        Self {
            sport,
            dport,
            flag,
            seq,
            data: Vec::new(),
        }
    }
    pub fn new_init(sport: u16, dport: u16, flag: Flag) -> Self {
        Self {
            sport,
            dport,
            flag,
            seq: 0,
            data: Vec::new(),
        }
    }

    pub fn new_reply(frame: &Frame, flag: Flag, seq: u32) -> Self {
        Self {
            sport: frame.dport,
            dport: frame.sport,
            flag,
            seq,
            data: Vec::new(),
        }
    }

    pub fn new_data(sport: u16, dport: u16, seq: u32, data: &[u8]) -> Self {
        Self {
            sport,
            dport,
            flag: Flag::Unset,
            seq,
            data: Vec::from(data),
        }
    }
}

impl From<Frame> for Message {
    #[tracing::instrument(skip_all, level = "trace")]
    fn from(mut frame: Frame) -> Message {
        let size = std::mem::size_of::<u16>()
            + std::mem::size_of::<u16>()
            + std::mem::size_of::<Flag>()
            + std::mem::size_of::<u32>()
            + frame.data.len();
        let mut encoded = Vec::with_capacity(size);
        encoded.extend_from_slice(&frame.sport.to_be_bytes());
        encoded.extend_from_slice(&frame.dport.to_be_bytes());
        encoded.extend_from_slice(&(frame.flag as u8).to_be_bytes());
        encoded.extend_from_slice(&frame.seq.to_be_bytes());
        encoded.append(&mut frame.data);
        Message::Binary(encoded)
    }
}

impl TryFrom<Message> for Frame {
    type Error = std::array::TryFromSliceError;
    #[tracing::instrument(skip_all, level = "trace")]
    fn try_from(value: Message) -> Result<Self, Self::Error> {
        match value {
            // TODO: Note that we do not correctly implement RFC6455, which requires
            // handling of control frames.
            Message::Binary(data) => {
                let mut data = bytes::Bytes::from(data);
                let sport = data.get_u16();
                let dport = data.get_u16();
                let flag = match data.get_u8() {
                    0 => Flag::Syn,
                    1 => Flag::SynAck,
                    2 => Flag::Ack,
                    3 => Flag::Rst,
                    4 => Flag::Fin,
                    5 => Flag::Unset,
                    _ => {
                        warn!("Invalid flag value");
                        Flag::Unset
                    }
                };
                let seq = data.get_u32();
                Ok(Self {
                    sport,
                    dport,
                    flag,
                    seq,
                    data: Vec::from(data),
                })
            }
            _ => unreachable!("Only binary messages should be sent by client"),
        }
    }
}
