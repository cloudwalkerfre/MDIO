
// mdio_core/src/vfs.rs

use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub enum MountMode { RW, RO }

#[derive(Serialize)]
pub struct FileNode {
    pub path: String,
    pub id: u32,
}
