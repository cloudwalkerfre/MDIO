use anyhow::{Result, anyhow};
use std::convert::TryFrom;
use serde::{Serialize, Deserialize};
use cola::{Insertion, Deletion};

#[derive(Serialize, Deserialize, Clone)]
pub enum WireOp {
    Insert {
        op: Insertion,
        content: String,
    },
    Delete {
        op: Deletion,
    },
}

// Upstream (WASM -> Server)
pub const NET_HEADER_SIZE: usize = 13; // Tag(1)+Len(4)+Peer(4)+Doc(4)

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NTag {
    UpTree = 0x01,
    UpOpen = 0x02,
    UpClose = 0x03,
    UpSync = 0x04,
    // exec 通路（WASM 透传，peer=0/doc=0 系统帧）——响应复用 DnExec=0x85
    UpExec = 0x0E,
    TelemetryProbe = 0x30,
    DnTree = 0x81,
    DnOpen = 0x82,
    DnSync = 0x83,
    DnAst = 0x84,
    DnExec = 0x85,
    TelemetryReturn = 0xB0,
    DnErr = 0xFF,
}

impl TryFrom<u8> for NTag {
    type Error = anyhow::Error;
    fn try_from(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(NTag::UpTree),
            0x02 => Ok(NTag::UpOpen),
            0x03 => Ok(NTag::UpClose),
            0x04 => Ok(NTag::UpSync),
            0x0E => Ok(NTag::UpExec),
            0x30 => Ok(NTag::TelemetryProbe),
            0x81 => Ok(NTag::DnTree),
            0x82 => Ok(NTag::DnOpen),
            0x83 => Ok(NTag::DnSync),
            0x84 => Ok(NTag::DnAst),
            0x85 => Ok(NTag::DnExec),
            0xB0 => Ok(NTag::TelemetryReturn),
            0xFF => Ok(NTag::DnErr),
            _ => Err(anyhow!("Unknown NTag: 0x{:02X}", value)),
        }
    }
}
