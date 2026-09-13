use axum::http::HeaderMap;
use sbx::webdav::{
    Depth, IfCondition, parse_clark, parse_depth, parse_destination, parse_if_header,
    parse_lock_token_header, parse_overwrite, parse_timeout,
};
use std::time::Duration;

#[test]
fn test_parse_if_simple_token() {
    let mut h = HeaderMap::new();
    h.insert("if", "(<opaquelocktoken:t1>)".parse().unwrap());
    let lists = parse_if_header(&h);
    assert_eq!(lists.len(), 1);
    assert_eq!(
        lists[0].conditions[0],
        IfCondition::StateToken("opaquelocktoken:t1".into())
    );
}

#[test]
fn test_parse_if_not() {
    let mut h = HeaderMap::new();
    h.insert("if", "(Not <DAV:no-lock>)".parse().unwrap());
    let lists = parse_if_header(&h);
    assert_eq!(
        lists[0].conditions[0],
        IfCondition::Not(Box::new(IfCondition::StateToken("DAV:no-lock".into())))
    );
}

#[test]
fn test_parse_if_resource_tag() {
    let mut h = HeaderMap::new();
    h.insert("if", "</path> (<opaquelocktoken:t1>)".parse().unwrap());
    let lists = parse_if_header(&h);
    assert_eq!(lists[0].resource_tag, Some("/path".into()));
}

#[test]
fn test_parse_if_empty_header() {
    let h = HeaderMap::new();
    let lists = parse_if_header(&h);
    assert!(lists.is_empty());
}

#[test]
fn test_parse_if_multiple_lists_same_resource_tag() {
    let mut h = HeaderMap::new();
    h.insert(
        "if",
        "</a> (<opaquelocktoken:t1>) (<opaquelocktoken:t2>)"
            .parse()
            .unwrap(),
    );
    let lists = parse_if_header(&h);
    assert_eq!(lists.len(), 2);
    assert_eq!(lists[0].resource_tag, Some("/a".into()));
    assert_eq!(lists[1].resource_tag, Some("/a".into()));
    assert_eq!(
        lists[0].conditions[0],
        IfCondition::StateToken("opaquelocktoken:t1".into())
    );
    assert_eq!(
        lists[1].conditions[0],
        IfCondition::StateToken("opaquelocktoken:t2".into())
    );
}

#[test]
fn test_parse_if_multiple_conditions() {
    let mut h = HeaderMap::new();
    h.insert(
        "if",
        "(<opaquelocktoken:t1> <opaquelocktoken:t2>)"
            .parse()
            .unwrap(),
    );
    let lists = parse_if_header(&h);
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].conditions.len(), 2);
    assert_eq!(
        lists[0].conditions[0],
        IfCondition::StateToken("opaquelocktoken:t1".into())
    );
    assert_eq!(
        lists[0].conditions[1],
        IfCondition::StateToken("opaquelocktoken:t2".into())
    );
}

#[test]
fn test_parse_if_standalone_token() {
    let mut h = HeaderMap::new();
    h.insert("if", "<opaquelocktoken:t1>".parse().unwrap());
    let lists = parse_if_header(&h);
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].resource_tag, None);
    assert_eq!(
        lists[0].conditions[0],
        IfCondition::StateToken("opaquelocktoken:t1".into())
    );
}

#[test]
fn test_parse_if_multiple_no_tag_lists() {
    let mut h = HeaderMap::new();
    h.insert(
        "if",
        "(<opaquelocktoken:t1>) (<opaquelocktoken:t2>)"
            .parse()
            .unwrap(),
    );
    let lists = parse_if_header(&h);
    assert_eq!(lists.len(), 2);
    assert!(lists.iter().all(|l| l.resource_tag.is_none()));
    assert_eq!(
        lists[0].conditions[0],
        IfCondition::StateToken("opaquelocktoken:t1".into())
    );
    assert_eq!(
        lists[1].conditions[0],
        IfCondition::StateToken("opaquelocktoken:t2".into())
    );
}

#[test]
fn test_parse_if_unclosed_angle_bracket_no_panic() {
    let mut h = HeaderMap::new();
    h.insert("if", "(<opaquelocktoken:t1".parse().unwrap());
    let lists = std::panic::catch_unwind(|| parse_if_header(&h));
    assert!(lists.is_ok(), "unclosed angle bracket should not panic");
}

#[test]
fn test_parse_if_unclosed_paren_no_panic() {
    let mut h = HeaderMap::new();
    h.insert("if", "(<opaquelocktoken:t1>".parse().unwrap());
    let lists = std::panic::catch_unwind(|| parse_if_header(&h));
    assert!(lists.is_ok(), "unclosed parenthesis should not panic");
}

#[test]
fn test_parse_if_not_and_token() {
    let mut h = HeaderMap::new();
    h.insert(
        "if",
        "(Not <opaquelocktoken:t1> <opaquelocktoken:t2>)"
            .parse()
            .unwrap(),
    );
    let lists = parse_if_header(&h);
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].conditions.len(), 2);
    assert_eq!(
        lists[0].conditions[0],
        IfCondition::Not(Box::new(IfCondition::StateToken(
            "opaquelocktoken:t1".into()
        )))
    );
    assert_eq!(
        lists[0].conditions[1],
        IfCondition::StateToken("opaquelocktoken:t2".into())
    );
}

#[test]
fn test_parse_lock_token_header() {
    let mut h = HeaderMap::new();
    h.insert("lock-token", "<opaquelocktoken:abc>".parse().unwrap());
    assert_eq!(parse_lock_token_header(&h).unwrap(), "opaquelocktoken:abc");
}

#[test]
fn test_parse_timeout_seconds() {
    let mut h = HeaderMap::new();
    h.insert("timeout", "Second-3600".parse().unwrap());
    assert_eq!(parse_timeout(&h), Some(Duration::from_secs(3600)));
}

#[test]
fn test_parse_depth() {
    let h = HeaderMap::new();
    assert_eq!(parse_depth(&h), Depth::Infinity);

    let mut h = HeaderMap::new();
    h.insert("depth", "0".parse().unwrap());
    assert_eq!(parse_depth(&h), Depth::Zero);

    let mut h = HeaderMap::new();
    h.insert("depth", "1".parse().unwrap());
    assert_eq!(parse_depth(&h), Depth::One);
}

#[test]
fn test_parse_destination_full_url() {
    let mut h = HeaderMap::new();
    h.insert(
        "destination",
        "http://localhost:8080/docs/file.txt".parse().unwrap(),
    );
    assert_eq!(parse_destination(&h).unwrap(), "/docs/file.txt");
}

#[test]
fn test_parse_destination_relative() {
    let mut h = HeaderMap::new();
    h.insert("destination", "/docs/file.txt".parse().unwrap());
    assert_eq!(parse_destination(&h).unwrap(), "/docs/file.txt");
}

#[test]
fn test_parse_destination_strips_trailing_slash() {
    let mut h = HeaderMap::new();
    h.insert("destination", "/docs/".parse().unwrap());
    assert_eq!(parse_destination(&h).unwrap(), "/docs");
}

#[test]
fn test_parse_overwrite_default() {
    assert!(parse_overwrite(&HeaderMap::new()));
}

#[test]
fn test_parse_overwrite_false() {
    let mut h = HeaderMap::new();
    h.insert("overwrite", "F".parse().unwrap());
    assert!(!parse_overwrite(&h));
}

#[test]
fn test_parse_clark_function() {
    assert_eq!(
        parse_clark("{http://example.com}prop0"),
        Some(("http://example.com", "prop0"))
    );
    assert_eq!(parse_clark("prop0"), Some(("", "prop0")));
    assert_eq!(parse_clark("{broken"), None);
}
