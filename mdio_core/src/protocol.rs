// 协议头长度: Tag(1) + Len(4) + Peer(4) + Doc(4) = 13
pub const HEADER_SIZE: usize = 13;

// Tag 定义
pub const UP_TREE: u8 = 0x01;
pub const UP_OPEN: u8 = 0x02;
pub const UP_CLOSE: u8 = 0x03;
pub const UP_SYNC: u8 = 0x04;
pub const UP_EXEC: u8 = 0x05;
pub const UP_FILE: u8 = 0x06;

pub const DN_TREE: u8 = 0x81;
pub const DN_OPEN: u8 = 0x82;
pub const DN_SYNC: u8 = 0x83;
pub const DN_AST: u8 = 0x84;
pub const DN_EXEC: u8 = 0x85;
pub const DN_FILE: u8 = 0x86;
pub const DN_ERR: u8 = 0xFF;

// ── tele_gateway 探针 ──
pub const TELEMETRY_PROBE:  u8 = 0x30;
pub const TELEMETRY_RETURN: u8 = 0xB0;

/// 文件/文件夹操作
/// Payload: bincode(FileOp)，帧头 pipe 字段 = crc32(entry_id)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum FileOp {
    New  { relative_path: String, checkpoint: Vec<u8> },
    Del  { relative_path: String },
    Mov  { old_path: String, new_path: String },
}