// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Search over names, tags, sets and categories.
//!
//! Every query word must match the start of some word of the item (AND
//! semantics); hits are ranked so that whole-name matches come first, then
//! name-word matches, then tags, then set and category words.

use crate::item::{Index, Item};
use std::path::PathBuf;

/// One search result.
#[derive(Clone, Debug)]
pub struct Hit<'a> {
    pub item: &'a Item,
    pub index: &'a Index,
    pub score: u32,
}

impl Hit<'_> {
    /// Absolute path of the item's main file.
    pub fn path(&self) -> PathBuf {
        self.index.path(&self.item.file)
    }
}

/// Lower-case alphanumeric words (apostrophes dropped, `&` ignored).
pub fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        } else if c == '\'' {
            // "o'clock" stays one word
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Scores one item against query tokens; `None` when a token matches nothing.
pub fn score(item: &Item, query: &[String]) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    let name_words = tokens(&item.name);
    let name_joined = name_words.join(" ");
    let query_joined = query.join(" ");
    let mut total = 0u32;
    if name_joined == query_joined {
        total += 1000;
    } else if name_joined.starts_with(&query_joined) {
        total += 300;
    }
    for q in query {
        let mut best = 0u32;
        for w in &name_words {
            if w == q {
                best = best.max(100);
            } else if w.starts_with(q.as_str()) {
                best = best.max(60);
            }
        }
        if best < 100 {
            for t in &item.tags {
                for w in tokens(t) {
                    if w == *q {
                        best = best.max(40);
                    } else if w.starts_with(q.as_str()) {
                        best = best.max(20);
                    }
                }
            }
        }
        if best < 40 {
            for w in tokens(&item.set).into_iter().chain(tokens(&item.category)) {
                if w == *q {
                    best = best.max(30);
                } else if w.starts_with(q.as_str()) {
                    best = best.max(15);
                }
            }
        }
        if best == 0 {
            return None;
        }
        total += best;
    }
    Some(total)
}

/// Searches across the given indexes. An empty query returns everything in
/// index order.
pub fn search<'a>(indexes: impl Iterator<Item = &'a Index>, query: &str) -> Vec<Hit<'a>> {
    let q = tokens(query);
    let mut hits: Vec<Hit<'a>> = Vec::new();
    for index in indexes {
        for item in &index.items {
            if let Some(score) = score(item, &q) {
                hits.push(Hit { item, index, score });
            }
        }
    }
    if !q.is_empty() {
        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.item.name.cmp(&b.item.name)));
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{Details, Kind, Licence, Source};

    fn item(name: &str, tags: &[&str], set: &str, category: &str) -> Item {
        Item {
            id: name.to_lowercase().replace(' ', "-"),
            name: name.into(),
            kind: Kind::Sticker,
            category: category.into(),
            set: set.into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            file: PathBuf::from(format!("{name}.png")),
            preview: None,
            licence: Licence::Studio,
            attribution: String::new(),
            source: Source::studio(),
            details: Details::default(),
        }
    }

    #[test]
    fn tokenises() {
        assert_eq!(tokens("Grinning Face & O'Clock 3D"), vec!["grinning", "face", "oclock", "3d"]);
    }

    #[test]
    fn ranks_name_over_tags_over_set() {
        let mut ix = Index::new("/tmp");
        ix.items.push(item("Cat 01", &["kitten"], "Animal Memes", "Animals"));
        ix.items.push(item("Dog 01", &["cat", "lover"], "Animal Memes", "Animals"));
        ix.items.push(item("Hat 01", &[], "Cat Hats", "Clothes"));
        ix.items.push(item("Car 01", &[], "Vehicles", "Travel"));
        let hits = ix.search("cat");
        let names: Vec<&str> = hits.iter().map(|h| h.item.name.as_str()).collect();
        assert_eq!(names, vec!["Cat 01", "Dog 01", "Hat 01"]);
        assert!(ix.search("ca").iter().any(|h| h.item.name == "Car 01"));
        assert!(ix.search("cat hat").iter().all(|h| h.item.name == "Hat 01"));
        assert!(ix.search("zebra").is_empty());
        assert_eq!(ix.search("").len(), 4);
        assert_eq!(hits[0].path(), PathBuf::from("/tmp/Cat 01.png"));
    }

    #[test]
    fn exact_name_wins() {
        let mut ix = Index::new("/");
        ix.items.push(item("Party Hat 02", &["party"], "Party", "Party"));
        ix.items.push(item("Party", &[], "Party", "Party"));
        assert_eq!(ix.search("party").first().unwrap().item.name, "Party");
    }
}
