use std::sync::Arc;

use lectern_core::cache::{CacheKey, RenderCache};
use lectern_core::render::{RenderedDoc, TaskStats};

fn doc(title: &str) -> Arc<RenderedDoc> {
    Arc::new(RenderedDoc {
        html: format!("<h1>{title}</h1>"),
        outline: vec![],
        frontmatter: None,
        tasks: TaskStats::default(),
        title: title.to_owned(),
        word_count: 1,
        has_unresolved_wikilinks: false,
    })
}

fn key(path: &str, mtime_ms: i64) -> CacheKey {
    CacheKey {
        path: path.to_owned(),
        mtime_ms,
        size: 10,
        version: 1,
    }
}

fn title(cache: &mut RenderCache, k: &CacheKey) -> Option<String> {
    cache.get(k).map(|d| d.title.clone())
}

#[test]
fn cache_lru() {
    let mut cache = RenderCache::new(2);
    cache.put(key("a", 1), doc("A"));
    cache.put(key("b", 1), doc("B"));
    assert_eq!(title(&mut cache, &key("a", 1)).as_deref(), Some("A"));
    cache.put(key("c", 1), doc("C"));
    assert!(
        cache.get(&key("b", 1)).is_none(),
        "b was least recently used"
    );
    assert_eq!(title(&mut cache, &key("a", 1)).as_deref(), Some("A"));
    assert_eq!(title(&mut cache, &key("c", 1)).as_deref(), Some("C"));
}

#[test]
fn any_key_change_misses() {
    let mut cache = RenderCache::new(4);
    cache.put(key("a", 1), doc("A"));
    assert!(cache.get(&key("a", 2)).is_none());
    assert!(cache
        .get(&CacheKey {
            size: 11,
            ..key("a", 1)
        })
        .is_none());
    assert!(cache
        .get(&CacheKey {
            version: 2,
            ..key("a", 1)
        })
        .is_none());
    assert!(cache.get(&key("a", 1)).is_some());
}

#[test]
fn a_new_render_of_a_path_replaces_the_old_one() {
    let mut cache = RenderCache::new(2);
    cache.put(key("a", 1), doc("A1"));
    cache.put(key("b", 1), doc("B"));
    cache.put(key("a", 2), doc("A2"));
    assert!(cache.get(&key("a", 1)).is_none());
    // The stale render was dropped rather than evicting `b`.
    assert_eq!(title(&mut cache, &key("b", 1)).as_deref(), Some("B"));
    assert_eq!(title(&mut cache, &key("a", 2)).as_deref(), Some("A2"));
}

#[test]
fn put_same_key_replaces_value() {
    let mut cache = RenderCache::new(2);
    cache.put(key("a", 1), doc("old"));
    cache.put(key("a", 1), doc("new"));
    cache.put(key("b", 1), doc("B"));
    assert_eq!(title(&mut cache, &key("a", 1)).as_deref(), Some("new"));
    assert_eq!(title(&mut cache, &key("b", 1)).as_deref(), Some("B"));
}

#[test]
fn zero_capacity_stores_nothing() {
    let mut cache = RenderCache::new(0);
    cache.put(key("a", 1), doc("A"));
    assert!(cache.get(&key("a", 1)).is_none());
}
