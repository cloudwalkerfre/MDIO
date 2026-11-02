#[cfg(test)]
mod html_integration_tests {
    use super::*;
    use crate::engine::BlockProj;
    use crate::selector::{select_nodes, Step, FilterExpr, CmpOp, Value, TypeName};
    use crate::projection::project_text;

    fn html_tag(tag: &str) -> Vec<Step> {
        // 树化适配:旧写法 [RecursiveWildcard, TypeCheck(Html),
        // tag==X] 依赖遍历可见性——树化后 RecursiveWildcard 容器穿透(容器
        // 不可见——MD 2.2 遍历语义——iter20b 实证)——HtmlBlock 壳不可见。
        // HTML 元素寻址 = 寻址语义(RecursiveDrill——全访问——壳 tag 可达——
        // 身份诚实——cf lang/hb tagname 属于容器——属性归属)。
        vec![Step::RecursiveDrill(tag.to_string())]
    }

    // ── Top-level HtmlBlock selection ──

    #[test]
    fn test_html_top_tag() {
        let snap = project_text("<div>\ntext\n</div>\n");
        let nodes = select_nodes(&snap, &html_tag("div"));
        assert_eq!(nodes.len(), 1);
        assert_eq!(snap.read_block_proj(&nodes[0], BlockProj::Tag), "div");
    }

    #[test]
    fn test_html_top_mark() {
        let snap = project_text("<div>\ntext\n</div>\n");
        let nodes = select_nodes(&snap, &html_tag("div"));
        assert_eq!(nodes.len(), 1);
        assert!(snap.read_block_proj(&nodes[0], BlockProj::Mark).starts_with("<div"));
    }

    #[test]
    fn test_html_top_content() {
        let snap = project_text("<div>\ntext\n</div>\n");
        let nodes = select_nodes(&snap, &html_tag("div"));
        assert_eq!(nodes.len(), 1);
        let content = snap.read_block_proj(&nodes[0], BlockProj::Content);
        assert!(content.contains("text"));
    }

    #[test]
    fn test_html_close_open() {
        let snap = project_text("<div>\n<p>old</p>\n</div><p>new</p>\n");
        let all = select_nodes(&snap, &html_tag("div"));
        assert!(!all.is_empty(), "top-level <div> should be found");
    }

    #[test]
    fn test_html_void() {
        let snap = project_text("<div>\n<br>\n<hr>\n<img src='x.png'/>\n</div>\n");
        let nodes = select_nodes(&snap, &html_tag("div"));
        assert_eq!(nodes.len(), 1);
        assert_eq!(snap.read_block_proj(&nodes[0], BlockProj::Tag), "div");
    }

    // ── Direct children via Drill+Wildcard ──
    // 树化适配:HtmlBlock 内 HtmlElement 行并入块区间
    // (块级真相——渲染同款——hb 块 ct 含内部元素文本)——内部元素经
    // Content 投影读取(嵌套投影模式)。

    #[test]
    fn test_html_children() {
        let snap = project_text("<div>\n<p>Hello</p>\n<img src='x.png'/>\n</div>\n");
        let divs = select_nodes(&snap, &html_tag("div"));
        assert_eq!(divs.len(), 1);
        let content = snap.read_block_proj(&divs[0], BlockProj::Content);
        assert!(content.contains("<p>Hello</p>"), "div 内容应含 p 元素: {content:?}");
        assert!(content.contains("<img src='x.png'/>"), "div 内容应含 img 元素: {content:?}");
    }

    #[test]
    fn test_html_child_mark() {
        let snap = project_text("<div>\n<p>Hello</p>\n</div>\n");
        let divs = select_nodes(&snap, &html_tag("div"));
        assert_eq!(divs.len(), 1);
        assert!(snap.read_block_proj(&divs[0], BlockProj::Mark).starts_with("<div"));
        let content = snap.read_block_proj(&divs[0], BlockProj::Content);
        assert!(content.contains("<p>"), "div 内容应含 p 开标签: {content:?}");
    }

    #[test]
    fn test_html_child_content() {
        let snap = project_text("<div>\n<p>Hello</p>\n</div>\n");
        let divs = select_nodes(&snap, &html_tag("div"));
        assert_eq!(divs.len(), 1);
        let content = snap.read_block_proj(&divs[0], BlockProj::Content);
        assert!(content.contains("Hello"), "div 内容应含 p 文本: {content:?}");
    }

    #[test]
    fn test_html_child_frame() {
        let snap = project_text("<div>\n<span>\ntext\n</span>\n</div>\n");
        let divs = select_nodes(&snap, &html_tag("div"));
        assert_eq!(divs.len(), 1);
        let content = snap.read_block_proj(&divs[0], BlockProj::Content);
        assert!(content.contains("<span>"), "div 内容应含 span 开标签: {content:?}");
        assert!(content.contains("text"), "div 内容应含 span 文本: {content:?}");
    }

    #[test]
    fn test_html_child_custom() {
        let snap = project_text("<div>\n<my-component/>\n</div>\n");
        let divs = select_nodes(&snap, &html_tag("div"));
        assert_eq!(divs.len(), 1);
        let content = snap.read_block_proj(&divs[0], BlockProj::Content);
        assert!(content.contains("<my-component/>"), "div 内容应含自定义元素: {content:?}");
    }

    // ── to_ast ──

    #[test]
    fn test_html_to_ast() {
        let snap = project_text("<div>\n<p>Hello</p>\n</div>\n");
        let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
        assert!(json.contains("hb") || json.contains("he"), "nested to_ast: {}", json);
    }

    // ── Memory read ($$) ──

    #[test]
    fn test_html_mem_read() {
        let outer = project_text("<div>\n<p>Hello</p>\n</div>\n");
        let divs = select_nodes(&outer, &html_tag("div"));
        assert!(!divs.is_empty());
        let content = outer.read_block_proj(&divs[0], BlockProj::Content);
        assert!(content.contains("<p>Hello</p>"), "content should contain inner element");
        // Project inner content and verify it's valid — it's a <p> element
        let inner = project_text(&content);
        let inner_ps = select_nodes(&inner, &html_tag("p"));
        assert!(!inner_ps.is_empty());
    }
}
