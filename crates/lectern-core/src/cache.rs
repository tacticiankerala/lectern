//! Recently rendered documents, so switching back to one skips the render.

use std::collections::HashMap;
use std::sync::Arc;

use crate::render::RenderedDoc;

/// Identifies one render: a change to the file or to the renderer misses the cache.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct CacheKey {
    pub path: String,
    pub mtime_ms: i64,
    pub size: u64,
    /// `render::RENDER_VERSION` when the document was rendered.
    pub version: u32,
}

/// A least-recently-used cache of renders (64 entries in the app): each the rendered document, or
/// whatever the app keeps with it (`V`), such as the text it was rendered from.
pub struct RenderCache<V = Arc<RenderedDoc>> {
    cap: usize,
    /// Each render with the tick it was last used at.
    entries: HashMap<CacheKey, (V, u64)>,
    tick: u64,
}

impl<V: Clone> RenderCache<V> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            entries: HashMap::with_capacity(cap),
            tick: 0,
        }
    }

    /// The render for `k`, which becomes the most recently used.
    pub fn get(&mut self, k: &CacheKey) -> Option<V> {
        self.tick += 1;
        let (render, used) = self.entries.get_mut(k)?;
        *used = self.tick;
        Some(render.clone())
    }

    /// Stores `v` as the most recently used. Other renders of the same path are dropped, since the
    /// file has changed since; beyond capacity, the least recently used render goes.
    pub fn put(&mut self, k: CacheKey, v: V) {
        if self.cap == 0 {
            return;
        }
        self.entries
            .retain(|old, _| old.path != k.path || *old == k);
        if !self.entries.contains_key(&k) && self.entries.len() >= self.cap {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        self.tick += 1;
        self.entries.insert(k, (v, self.tick));
    }
}
