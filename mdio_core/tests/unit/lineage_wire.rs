// 血统线格式判官 —— 上行/下行载荷的 [lineage: u64 LE][bincode(Vec<WireOp>)] 约定
//
// 判官：① 带号的载荷解得出号，且纯 op 字节逐字节等于原样；
//       ② 不带号的**真实**旧载荷不能被误读成"血统号 = 前 8 字节"；
//       ③ 全零尾巴的边界载荷同样不能（只判"decode 成功"会在这里翻车：
//          零尾巴被解成空 op 表并消耗 1 字节就返回成功）；
//       ④ 短载荷（< 8 字节）不越界。
//
// 为什么必须钉住格式：血统号是"这份副本还对不对得上"的唯一判据
// 前缀漏掉或误读，
// 症状不是报错，而是 **integrate None 进 pending 永久静默**。
use crate::session::WireOp;
use crate::store::ProjectStore;
use cola::Replica;

/// 一段**真实**的 op 载荷：一条插入 → bincode(Vec<WireOp>)。
fn real_ops() -> Vec<u8> {
    let mut rep = Replica::new(7u64, 0);
    let ins = rep.inserted(0, 5);
    let v = vec![WireOp::Insert { op: ins, content: "hello".to_string() }];
    bincode::serde::encode_to_vec(&v, crate::session::bincode_config()).unwrap()
}

#[test]
fn stamped_payload_yields_lineage_and_clean_ops() {
    let body = real_ops();
    assert!(body.len() > 8, "样本载荷本身要够长才有判别意义（实长 {}）", body.len());
    let stamped = ProjectStore::stamp_lineage(0xDEAD_BEEF_1234_5678, &body);
    let (l, got) = ProjectStore::split_lineage(&stamped);
    assert_eq!(l, Some(0xDEAD_BEEF_1234_5678), "带号载荷必须解出血统号");
    assert_eq!(got, &body[..], "解出的 op 字节必须与原样逐字节相同");
}

#[test]
fn legacy_payload_is_not_misread_as_lineage() {
    // 旧客户端只发 bincode(Vec<WireOp>)，没有前缀。若前 8 字节被当成血统号，
    // 服务端就会拿一个随机数去比对血统 ⇒ 误判换代 ⇒ 无谓重取（甚至丢编辑）。
    let body = real_ops();
    let (l, got) = ProjectStore::split_lineage(&body);
    assert_eq!(l, None, "无前缀载荷必须判为『无血统号』而不是硬读前 8 字节");
    assert_eq!(got, &body[..], "无前缀时原样返回，交给 apply_update 自行解码");
}

#[test]
fn zero_tail_payload_is_not_misread_as_lineage() {
    // 边界：≥8 字节但后缀全是 0。只判"decode 成功"的实现会在这里返回
    // Some(空 op 表消耗 1 字节)，把旧载荷误判成带号——所以判据包含"恰好吃光后缀"。
    let mut body = bincode::serde::encode_to_vec(&Vec::<WireOp>::new(), crate::session::bincode_config()).unwrap();
    while body.len() < 32 { body.push(0); }
    let (l, got) = ProjectStore::split_lineage(&body);
    assert_eq!(l, None, "全零尾巴不是合法前缀格式");
    assert_eq!(got, &body[..]);
}

#[test]
fn short_payload_does_not_panic() {
    let (l, got) = ProjectStore::split_lineage(&[1, 2, 3]);
    assert_eq!(l, None);
    assert_eq!(got, &[1u8, 2, 3][..]);
}
