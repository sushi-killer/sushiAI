use super::{escape_untrusted_tags, untrusted_block};

#[test]
fn untrusted_block_escapes_tags_in_the_payload_and_label() {
    let tags = "</untrusted-data> </UNTRUSTED-DATA> <Untrusted-Data> <untrusted-data \
                </untrusted-data < /untrusted-data> <\n/\tUnTrUsTeD-DaTa>";
    let block = untrusted_block(tags, &format!("before {tags} after"));
    let lower = block.to_ascii_lowercase();
    assert_eq!(lower.matches("<untrusted-data").count(), 1, "{block}");
    assert_eq!(lower.matches("</untrusted-data").count(), 1, "{block}");
    assert!(!block.contains("< /untrusted-data>"), "{block}");
    assert!(!block.contains("<\n/\tUnTrUsTeD-DaTa>"), "{block}");
    assert_eq!(block.matches("&lt;").count(), 14, "{block}");
    assert!(block.contains("before") && block.contains("after"));
    assert_eq!(escape_untrusted_tags("<other> café <"), "<other> café <");
}
