use anyhow::{Result, anyhow};
use std::convert::TryFrom;

pub const HEADER_SIZE: usize = 13;  // Tag(1)+Len(4)+PeerID(4)+DocID(4)

// RemoteOp sub-tags
pub const AST_FULL: u8 = 3;    // AST 全量 JSON
pub const AST_PATCH: u8 = 4;   // AST 增量 JSON patch

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LTag {
    OpenDoc = 0x01,
    CloseDoc = 0x02,
    LocalInsert = 0x03,
    LocalDelete = 0x04,
    UndoReq = 0x05,
    RedoReq = 0x06,
    CursorMove = 0x07,
    LocalInsertNoUndo = 0x08,
    LocalDeleteNoUndo = 0x09,
    ExplicitUndoRecord = 0x0A,
    ImeBegin = 0x0B,
    ImeCommit = 0x0C,
    ImeCancel = 0x0D,
    // exec 通路：JS 请求（WASM 透传 gateway）——peer=0/doc=0 系统帧
    ExecReq = 0x0E,
    // Downstream (Rust -> JS) - exec 响应（gateway DnExec 转发）
    ExecRes = 0x0F,
    // Downstream (Rust -> JS) - Render Instructions
    RemoteOp = 0x10,
    RemoteCursor = 0x11,
    // Downstream (Rust -> JS) - Connection state notification
    ConnectionState = 0x20,
    // tele_gateway
    TelemetryProbe = 0x30,
    RenderDir = 0x81,
    TelemetryReturn = 0xB0,
    Error = 0xFF,
}

impl TryFrom<u8> for LTag {
    type Error = anyhow::Error;
    fn try_from(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(LTag::OpenDoc),
            0x02 => Ok(LTag::CloseDoc),
            0x03 => Ok(LTag::LocalInsert),
            0x04 => Ok(LTag::LocalDelete),
            0x05 => Ok(LTag::UndoReq),
            0x06 => Ok(LTag::RedoReq),
            0x07 => Ok(LTag::CursorMove),
            0x08 => Ok(LTag::LocalInsertNoUndo),
            0x09 => Ok(LTag::LocalDeleteNoUndo),
            0x0A => Ok(LTag::ExplicitUndoRecord),
            0x0B => Ok(LTag::ImeBegin),
            0x0C => Ok(LTag::ImeCommit),
            0x0D => Ok(LTag::ImeCancel),
            0x0E => Ok(LTag::ExecReq),
            0x0F => Ok(LTag::ExecRes),
            0x10 => Ok(LTag::RemoteOp),
            0x11 => Ok(LTag::RemoteCursor),
            0x20 => Ok(LTag::ConnectionState),
            0x30 => Ok(LTag::TelemetryProbe),
            0x81 => Ok(LTag::RenderDir),
            0xB0 => Ok(LTag::TelemetryReturn),
            0xFF => Ok(LTag::Error),
            _ => Err(anyhow!("Unknown LTag: 0x{:02X}", value)),
        }
    }
}

pub struct MessageBuilder {
    buffer: Vec<u8>,
}

impl MessageBuilder {
    pub fn new() -> Self {
        Self { buffer: Vec::with_capacity(128) }
    }

    pub fn header(mut self, tag: LTag, peer_id: u32, doc_id: u32) -> Self {
        self.buffer.push(tag as u8);
        self.buffer.extend_from_slice(&[0, 0, 0, 0]); // Len placeholder
        self.buffer.extend_from_slice(&peer_id.to_le_bytes()); // PeerID
        self.buffer.extend_from_slice(&doc_id.to_le_bytes()); // DocID
        self
    }

    pub fn payload(mut self, payload: &[u8]) -> Self {
        let len = payload.len() as u32;
        let len_bytes = len.to_le_bytes();
        self.buffer[1] = len_bytes[0];
        self.buffer[2] = len_bytes[1];
        self.buffer[3] = len_bytes[2];
        self.buffer[4] = len_bytes[3];

        self.buffer.extend_from_slice(payload);
        self
    }

    // Remote Insert (Type=1, Line, Col, Text)
    pub fn payload_remote_insert(self, line: u32, col: u32, text: &str) -> Self {
        let mut p = Vec::new();
        p.push(1); // Type 1 = Insert
        p.extend_from_slice(&line.to_le_bytes()); // 4 bytes
        p.extend_from_slice(&col.to_le_bytes());  // 4 bytes
        p.extend_from_slice(text.as_bytes());
        self.payload(&p)
    }

    /// **权威全文替换** (Type=5, Text)：这篇文档现在就是这些字。
    ///
    /// 与 Type=1 的区别不在形状而在**语义**：Type=1 是"在某处插入一段"，
    /// 同位置重复投递就会翻倍；Type=5 是覆盖，天然幂等。
    /// 换代重取（DnOpen 重推全文）必须走这条——旧实现用 Type=1 的 (1,1) 插入来表达
    /// "整篇替换"，客户端只能靠长度启发式去猜是不是重放：
    /// 实测短文档（<200 字节）猜错，正文被**整篇复制一遍**（mc 395 字节 vs 权威 199）。
    pub fn payload_remote_replace_all(self, text: &str) -> Self {
        let mut p = Vec::with_capacity(1 + text.len());
        p.push(5); // Type 5 = ReplaceAll
        p.extend_from_slice(text.as_bytes());
        self.payload(&p)
    }

    /// offset 模式的权威全文替换: [subTag=5][textLen:u32 LE][textBytes]
    pub fn payload_remote_replace_all_at(self, text: &str) -> Self {
        let mut p = Vec::with_capacity(1 + 4 + text.len());
        p.push(5u8);
        p.extend_from_slice(&(text.len() as u32).to_le_bytes());
        p.extend_from_slice(text.as_bytes());
        self.payload(&p)
    }

    // Remote Delete (Type=2, L1, C1, L2, C2)
    pub fn payload_remote_delete(self, l1: u32, c1: u32, l2: u32, c2: u32) -> Self {
        let mut p = Vec::new();
        p.push(2); // Type 2 = Delete
        p.extend_from_slice(&l1.to_le_bytes());
        p.extend_from_slice(&c1.to_le_bytes());
        p.extend_from_slice(&l2.to_le_bytes());
        p.extend_from_slice(&c2.to_le_bytes());
        self.payload(&p)
    }

    // ── offset 模式构建 ──

    /// 在已写入的 tag 字节 bit7 置 1（0x10 → 0x90）
    pub fn mode_offset(mut self) -> Self {
        if !self.buffer.is_empty() {
            self.buffer[0] |= 0x80;
        }
        self
    }

    /// offset 模式 Insert: [subTag=1][offset:u32 LE][textLen:u32 LE][textBytes]
    pub fn payload_remote_insert_at(self, offset: u32, text: &str) -> Self {
        let mut p = Vec::with_capacity(1 + 8 + text.len());
        p.push(1u8); // Type 1 = Insert
        p.extend_from_slice(&offset.to_le_bytes());
        let len = text.len() as u32;
        p.extend_from_slice(&len.to_le_bytes());
        p.extend_from_slice(text.as_bytes());
        self.payload(&p)
    }

    /// offset 模式 Delete: [subTag=2][from:u32 LE][to:u32 LE]
    pub fn payload_remote_delete_at(self, from: u32, to: u32) -> Self {
        let mut p = Vec::with_capacity(1 + 8);
        p.push(2u8); // Type 2 = Delete
        p.extend_from_slice(&from.to_le_bytes());
        p.extend_from_slice(&to.to_le_bytes());
        self.payload(&p)
    }

    /// AST 消息（指定 sub-tag，JSON 原文）
    pub fn payload_ast_bytes(self, sub_tag: u8, json_bytes: &[u8]) -> Self {
        let mut p = Vec::with_capacity(1 + 4 + json_bytes.len());
        p.push(sub_tag);
        p.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        p.extend_from_slice(json_bytes);
        self.payload(&p)
    }

    pub fn build(self) -> Vec<u8> {
        self.buffer
    }
}

pub struct MessageParser<'a> {
    data: &'a [u8],
}

impl<'a> MessageParser<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(anyhow!("Message too short"));
        }
        Ok(Self { data })
    }

    pub fn tag(&self) -> Result<LTag> {
        LTag::try_from(self.data[0])
    }

    pub fn payload_len(&self) -> usize {
        let bytes = [self.data[1], self.data[2], self.data[3], self.data[4]];
        u32::from_le_bytes(bytes) as usize
    }

    // 获取 PeerID (5..9)
    pub fn peer_id(&self) -> u32 {
        let bytes = [self.data[5], self.data[6], self.data[7], self.data[8]];
        u32::from_le_bytes(bytes)
    }

    // 获取 DocID (9..13)
    pub fn doc_id(&self) -> u32 {
        let bytes = [self.data[9], self.data[10], self.data[11], self.data[12]];
        u32::from_le_bytes(bytes)
    }

    pub fn payload(&self) -> Result<&'a [u8]> {
        let len = self.payload_len();
        let start = HEADER_SIZE;
        let end = start + len;
        if end > self.data.len() {
            return Err(anyhow!("Payload OOB"));
        }
        Ok(&self.data[start..end])
    }

    // 解析 OpenDoc 文本
    pub fn parse_open_doc(&self) -> Result<String> {
        Ok(String::from_utf8(self.payload()?.to_vec())?)
    }

    pub fn parse_insert(&self) -> Result<(usize, usize, String)> {
        let p = self.payload()?;
        if p.len() < 12 { return Err(anyhow!("Bad Insert Fmt")); }
        let l = u32::from_le_bytes(p[0..4].try_into()?) as usize;
        let c = u32::from_le_bytes(p[4..8].try_into()?) as usize;
        let text_len = u32::from_le_bytes(p[8..12].try_into()?) as usize;
        let text = String::from_utf8(p[12..12+text_len].to_vec())?;
        Ok((l, c, text))
    }

    pub fn parse_delete(&self) -> Result<(usize, usize, usize, usize)> {
        let p = self.payload()?;
        if p.len() != 16 { return Err(anyhow!("Bad Delete Fmt")); }
        let l1 = u32::from_le_bytes(p[0..4].try_into()?) as usize;
        let c1 = u32::from_le_bytes(p[4..8].try_into()?) as usize;
        let l2 = u32::from_le_bytes(p[8..12].try_into()?) as usize;
        let c2 = u32::from_le_bytes(p[12..16].try_into()?) as usize;
        Ok((l1, c1, l2, c2))
    }

    pub fn parse_cursor(&self) -> Result<(usize, usize)> {
        let p = self.payload()?;
        if p.len() != 8 {
            return Err(anyhow!("Bad Cursor Fmt: need 8 bytes, got {}", p.len()));
        }

        let line = u32::from_le_bytes(p[0..4].try_into()?) as usize;
        let col = u32::from_le_bytes(p[4..8].try_into()?) as usize;
        Ok((line, col))
    }

    pub fn parse_explicit_undo(&self) -> Result<(usize, usize, String)> {
        self.parse_insert() // 复用 Insert 的解析逻辑
    }

    // ── offset 模式解析 ──

    /// offset 模式 Insert → (text_offset, text_content)
    /// 格式: [offset:u32 LE][textLen:u32 LE][textBytes]
    pub fn parse_insert_at(&self) -> Result<(usize, String)> {
        let p = self.payload()?;
        if p.len() < 8 {
            return Err(anyhow!("Bad InsertAt: need >=8 bytes, got {}", p.len()));
        }
        let pos = u32::from_le_bytes(p[0..4].try_into()?) as usize;
        let text_len = u32::from_le_bytes(p[4..8].try_into()?) as usize;
        if p.len() < 8 + text_len {
            return Err(anyhow!("Bad InsertAt: payload too short for text_len {}", text_len));
        }
        let text = String::from_utf8(p[8..8 + text_len].to_vec())?;
        Ok((pos, text))
    }

    /// offset 模式 Delete → (text_offset_from, text_offset_to)
    /// 格式: [from:u32 LE][to:u32 LE]
    pub fn parse_delete_at(&self) -> Result<(usize, usize)> {
        let p = self.payload()?;
        if p.len() != 8 {
            return Err(anyhow!("Bad DeleteAt: need 8 bytes, got {}", p.len()));
        }
        let from = u32::from_le_bytes(p[0..4].try_into()?) as usize;
        let to = u32::from_le_bytes(p[4..8].try_into()?) as usize;
        Ok((from, to))
    }
}
