//! GitHub-style heading ids.

use std::collections::HashMap;

use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

/// Hands out unique heading slugs within one document, the way GitHub does: a repeated slug gets
/// `-1`, `-2`, … appended, skipping any suffixed slug already taken.
#[derive(Debug, Default)]
pub struct Slugger {
    /// Every slug handed out, with the last suffix used for it as a base.
    seen: HashMap<String, u32>,
}

impl Slugger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn slug(&mut self, text: &str) -> String {
        let base = slugify(text);
        let mut slug = base.clone();
        while self.seen.contains_key(&slug) {
            let n = self.seen.entry(base.clone()).or_insert(0);
            *n += 1;
            slug = format!("{base}-{n}");
        }
        self.seen.insert(slug.clone(), 0);
        slug
    }
}

/// Lowercases, keeps letters, marks, numbers and connector punctuation such as `_` (Unicode
/// categories L, M, N and Pc), keeps `-`, turns each space into `-` and drops everything else,
/// emoji and other whitespace included. This is github-slugger's rule.
pub fn slugify(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' | '-' => Some('-'),
            c if kept_in_slug(c) => Some(c),
            _ => None,
        })
        .collect()
}

fn kept_in_slug(c: char) -> bool {
    matches!(
        c.general_category_group(),
        GeneralCategoryGroup::Letter | GeneralCategoryGroup::Mark | GeneralCategoryGroup::Number
    ) || c.general_category() == GeneralCategory::ConnectorPunctuation
}
