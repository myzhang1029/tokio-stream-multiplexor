use serde::{Deserialize, Serialize};
use tungstenite::Message;

#[derive(Serialize, Deserialize, Debug)]
pub enum Flag {
    Syn,
    SynAck,
    Ack,
    Rst,
    Fin,
    Unset,
}

#[derive(Serialize, Deserialize)]
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

impl TryInto<Message> for Frame {
    type Error = bincode::Error;

    #[tracing::instrument(skip_all, level = "trace")]
    fn try_into(self) -> Result<Message, Self::Error> {
        let encoded = bincode::serialize(&self)?;
        Ok(Message::Binary(encoded))
    }
}

impl TryFrom<Message> for Frame {
    type Error = bincode::Error;

    #[tracing::instrument(skip_all, level = "trace")]
    fn try_from(value: Message) -> Result<Self, Self::Error> {
        match value {
            // TODO: Note that we do not correctly implement RFC6455, which requires
            // handling of control frames.
            Message::Binary(data) => bincode::deserialize(&data),
            _ => unreachable!("Only binary messages should be sent by client"),
        }
    }
}
