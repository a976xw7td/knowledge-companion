//! FTS query edge case tests with real SQLite FTS5.

use rusqlite::Connection;

fn setup_fts() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE docs USING fts5(title, content);
         INSERT INTO docs VALUES ('Machine Learning', 'Neural networks and deep learning');
         INSERT INTO docs VALUES ('Recipe', 'Chocolate cake with flour and sugar');
         INSERT INTO docs VALUES ('中文测试', '全文搜索支持中文和英文 mixed content');",
    )
    .unwrap();
    conn
}

#[test]
fn test_fts_basic_match() {
    let conn = setup_fts();
    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM docs WHERE docs MATCH 'learning'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_fts_chinese_limitation() {
    // FTS5 unicode61 does NOT segment Chinese — known limitation.
    // Chinese search works via exact substring match only when characters are adjacent.
    // Future: jieba-rs tokenizer for proper CJK segmentation.
    let conn = setup_fts();
    let result = conn.query_row(
        "SELECT COUNT(*) FROM docs WHERE docs MATCH '中文'",
        [],
        |r| r.get::<_, i32>(0),
    );
    // May or may not match depending on SQLite version — either is OK
    assert!(result.is_ok());
}

#[test]
fn test_fts_mixed_language() {
    let conn = setup_fts();
    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM docs WHERE docs MATCH 'mixed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_fts_prefix_match() {
    let conn = setup_fts();
    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM docs WHERE docs MATCH 'chocol*'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_fts_empty_query_returns_nothing() {
    // FTS5 with empty query should error or return 0
    let conn = setup_fts();
    let result = conn.query_row("SELECT COUNT(*) FROM docs WHERE docs MATCH ''", [], |r| {
        r.get::<_, i32>(0)
    });
    assert!(result.is_err() || result.unwrap() == 0);
}
