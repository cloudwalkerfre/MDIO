// WAL entry 编解码 / replay 纯逻辑测试

use super::super::*;

// ── WAL entry 编解码 ──────────────────────────────────────────────

#[test]
fn wal_entry_encode_decode_edit() {
    let rec = EditRecord { start: 10, end: 20, text: "hello".into() };
    let entry = WalEntry::Edit(rec);
    let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();

    let (decoded, _): (WalEntry, _) = bincode::serde::decode_from_slice(&payload, bincode_cfg()).unwrap();
    match decoded {
        WalEntry::Edit(r) => {
            assert_eq!(r.start, 10);
            assert_eq!(r.end, 20);
            assert_eq!(r.text, "hello");
        }
        _ => panic!("expected Edit"),
    }
}

#[test]
fn wal_entry_encode_decode_delete_record() {
    let rec = EditRecord { start: 5, end: 15, text: String::new() };
    let entry = WalEntry::Edit(rec);
    let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
    let (decoded, _): (WalEntry, _) = bincode::serde::decode_from_slice(&payload, bincode_cfg()).unwrap();
    match decoded {
        WalEntry::Edit(r) => {
            assert_eq!(r.start, 5);
            assert_eq!(r.end, 15);
            assert!(r.text.is_empty());
        }
        _ => panic!("expected Edit"),
    }
}

#[test]
fn wal_entry_encode_decode_flush_marker() {
    let entry = WalEntry::FlushMarker;
    let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
    let (decoded, _): (WalEntry, _) = bincode::serde::decode_from_slice(&payload, bincode_cfg()).unwrap();
    assert!(matches!(decoded, WalEntry::FlushMarker));
}

#[test]
fn wal_entry_wire_format() {
    let rec = EditRecord { start: 0, end: 0, text: "x".into() };
    let entry = WalEntry::Edit(rec);
    let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
    let len = payload.len() as u32;
    let mut crc = crc32fast::Hasher::new();
    crc.update(&payload);
    let crc_val = crc.finalize();

    let mut wire = Vec::new();
    wire.extend_from_slice(&len.to_le_bytes());
    wire.extend_from_slice(&WAL_MAGIC.to_le_bytes());
    wire.extend_from_slice(&crc_val.to_le_bytes());
    wire.extend_from_slice(&payload);

    let read_len = u32::from_le_bytes([wire[0], wire[1], wire[2], wire[3]]) as usize;
    assert_eq!(read_len, payload.len());
    let read_magic = u32::from_le_bytes([wire[4], wire[5], wire[6], wire[7]]);
    assert_eq!(read_magic, WAL_MAGIC);
    let read_crc = u32::from_le_bytes([wire[8], wire[9], wire[10], wire[11]]);
    assert_eq!(read_crc, crc_val);

    let (decoded, _): (WalEntry, _) = bincode::serde::decode_from_slice(&wire[12..], bincode_cfg()).unwrap();
    assert!(matches!(decoded, WalEntry::Edit(_)));
}

// ── replay_wal_entries ────────────────────────────────────────────

#[test]
fn replay_wal_entries_full_replay() {
    let mut wire = Vec::new();
    for (s, e, t) in [(0usize, 0usize, "# A\n"), (4, 4, "B\n")] {
        let rec = EditRecord { start: s, end: e, text: t.into() };
        let entry = WalEntry::Edit(rec);
        let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
        let len = payload.len() as u32;
        wire.extend_from_slice(&len.to_le_bytes());
        wire.extend_from_slice(&WAL_MAGIC.to_le_bytes());
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&payload);
    }
    let result = replay_wal_entries(&wire, "");
    assert_eq!(result.unwrap(), "# A\nB\n");
}

#[test]
fn replay_wal_entries_skip_before_fm() {
    let mut wire = Vec::new();
    add_edit_to_wire(&mut wire, "old");
    add_fm_to_wire(&mut wire);
    add_edit_to_wire(&mut wire, "new");

    let result = replay_wal_entries(&wire, "");
    assert_eq!(result.unwrap(), "new");
}

#[test]
fn replay_wal_entries_no_fm_replays_all() {
    let mut wire = Vec::new();
    {
        let rec = EditRecord { start: 0, end: 0, text: "A".into() };
        let entry = WalEntry::Edit(rec);
        let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
        wire.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        wire.extend_from_slice(&WAL_MAGIC.to_le_bytes());
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&payload);
    }
    {
        let rec = EditRecord { start: 1, end: 1, text: "B".into() };
        let entry = WalEntry::Edit(rec);
        let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
        wire.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        wire.extend_from_slice(&WAL_MAGIC.to_le_bytes());
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&payload);
    }
    let result = replay_wal_entries(&wire, "");
    assert_eq!(result.unwrap(), "AB");
}

#[test]
fn replay_wal_entries_corrupt_magic_stops() {
    let mut wire = Vec::new();
    add_edit_to_wire(&mut wire, "A");
    wire.extend_from_slice(&5u32.to_le_bytes());
    wire.extend_from_slice(&0xDEADBEEFu32.to_le_bytes());
    wire.extend_from_slice(&0u32.to_le_bytes());
    wire.extend_from_slice(b"garbage");
    add_edit_to_wire(&mut wire, "B");
    let result = replay_wal_entries(&wire, "");
    assert_eq!(result.unwrap(), "A");
}

#[test]
fn replay_wal_entries_partial_at_end() {
    let mut wire = Vec::new();
    add_edit_to_wire(&mut wire, "ok");
    wire.extend_from_slice(&[0u8; 6]);
    let result = replay_wal_entries(&wire, "");
    assert_eq!(result.unwrap(), "ok");
}

#[test]
fn replay_wal_entries_empty_wal() {
    let result = replay_wal_entries(&[], "base");
    assert_eq!(result.unwrap(), "base");
}

#[test]
fn replay_wal_entries_with_base_content() {
    let mut wire = Vec::new();
    add_fm_to_wire(&mut wire);
    add_edit_to_wire(&mut wire, "world");
    let result = replay_wal_entries(&wire, "hello");
    assert_eq!(result.unwrap(), "worldhello");
}

#[test]
fn replay_wal_entries_delete_and_insert() {
    let mut wire = Vec::new();
    {
        let rec = EditRecord { start: 0, end: 3, text: String::new() };
        let entry = WalEntry::Edit(rec);
        let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
        wire.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        wire.extend_from_slice(&WAL_MAGIC.to_le_bytes());
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&payload);
    }
    {
        let rec = EditRecord { start: 0, end: 0, text: "new".into() };
        let entry = WalEntry::Edit(rec);
        let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
        wire.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        wire.extend_from_slice(&WAL_MAGIC.to_le_bytes());
        wire.extend_from_slice(&0u32.to_le_bytes());
        wire.extend_from_slice(&payload);
    }
    let result = replay_wal_entries(&wire, "old");
    assert_eq!(result.unwrap(), "new");
}

#[test]
fn sha256_hex_known_value() {
    let hash = sha256_hex("hello");
    assert_eq!(hash.len(), 64);
    assert_eq!(hash, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
}

// ── Helpers ────────────────────────────────────────────────────────

fn add_edit_to_wire(wire: &mut Vec<u8>, text: &str) {
    let rec = EditRecord { start: 0, end: 0, text: text.into() };
    let entry = WalEntry::Edit(rec);
    let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
    wire.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    wire.extend_from_slice(&WAL_MAGIC.to_le_bytes());
    wire.extend_from_slice(&0u32.to_le_bytes());
    wire.extend_from_slice(&payload);
}

fn add_fm_to_wire(wire: &mut Vec<u8>) {
    let entry = WalEntry::FlushMarker;
    let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
    wire.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    wire.extend_from_slice(&WAL_MAGIC.to_le_bytes());
    wire.extend_from_slice(&0u32.to_le_bytes());
    wire.extend_from_slice(&payload);
}
