// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Searchable help.
//!
//! The articles are messages in the catalogue (`help-<topic>-title`,
//! `-keys` for search words, `-body` for the text), so they are translated
//! like the rest of the interface. [`search`] looks in the person's
//! language and in English, so a query in either finds the article. The AI
//! editor reads the same articles through the `help` tool ([`answer`]).
//!
//! Press `F1` (or choose Help ▸ Search help) to open it; with a region
//! focused (`F6`) it opens on that region's article.

use crate::app::{Overlay, StudioApp};
use crate::icons::Icon;
use crate::kit::{self, Button};
use crate::theme;
use eframe::egui::{self, Context, Id, Vec2};
use serde_json::{json, Value};
use studio_i18n::{tr_in, Lang};
use crate::kit::ShowNoted;

pub struct Topic {
    pub id: &'static str,
    pub icon: Icon,
}

pub const TOPICS: &[Topic] = &[
    Topic { id: "start", icon: Icon::House },
    Topic { id: "templates", icon: Icon::Stack },
    Topic { id: "import", icon: Icon::DownloadSimple },
    Topic { id: "timeline", icon: Icon::FilmStrip },
    Topic { id: "split-trim", icon: Icon::Scissors },
    Topic { id: "titles", icon: Icon::TextT },
    Topic { id: "motion", icon: Icon::Path },
    Topic { id: "effects", icon: Icon::MagicWand },
    Topic { id: "colour", icon: Icon::Palette },
    Topic { id: "audio", icon: Icon::SpeakerHigh },
    Topic { id: "keyframes", icon: Icon::Diamond },
    Topic { id: "masks", icon: Icon::Selection },
    Topic { id: "tracking", icon: Icon::Aperture },
    Topic { id: "speed", icon: Icon::Timer },
    Topic { id: "captions", icon: Icon::Subtitles },
    Topic { id: "ai-editor", icon: Icon::Sparkle },
    Topic { id: "export", icon: Icon::Export },
    Topic { id: "workspaces", icon: Icon::SquaresFour },
    Topic { id: "shortcuts", icon: Icon::Keyboard },
    Topic { id: "keyboard-nav", icon: Icon::Cursor },
    Topic { id: "accessibility", icon: Icon::Eye },
    Topic { id: "language", icon: Icon::Monitor },
    Topic { id: "saving", icon: Icon::FolderOpen },
    Topic { id: "undo", icon: Icon::ArrowUUpLeft },
    Topic { id: "problems", icon: Icon::Warning },
];

/// Message ids every article needs.
pub fn message_ids() -> Vec<String> {
    TOPICS.iter().flat_map(|t| ["title", "keys", "body"].map(|k| format!("help-{}-{k}", t.id))).collect()
}

pub fn title(id: &str, lang: Lang) -> String {
    tr_in(lang, &format!("help-{id}-title"), &[])
}

pub fn body(id: &str, lang: Lang) -> String {
    tr_in(lang, &format!("help-{id}-body"), &[])
}

fn keys(id: &str, lang: Lang) -> String {
    tr_in(lang, &format!("help-{id}-keys"), &[])
}

/// Words of `s`, lower-cased; every CJK character counts as a word and
/// each pair of neighbours as one (there are no spaces to split on).
pub fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_cjk: Option<char> = None;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    for ch in s.chars() {
        let cjk = matches!(ch as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF);
        if cjk {
            flush(&mut cur, &mut out);
            out.push(ch.to_string());
            if let Some(p) = prev_cjk {
                out.push(format!("{p}{ch}"));
            }
            prev_cjk = Some(ch);
        } else if ch.is_alphanumeric() {
            prev_cjk = None;
            cur.extend(ch.to_lowercase());
        } else {
            prev_cjk = None;
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub id: &'static str,
    pub score: u32,
}

/// Words that carry no meaning in a question ('how do I cut a clip').
const STOPWORDS: &[&str] = &["how", "do", "does", "did", "i", "a", "an", "the", "in", "to", "of", "and", "or", "on", "is", "it", "my", "can", "what", "where", "when", "with", "for", "from", "this", "that", "me", "you", "your", "are", "be", "get", "make", "want"];

fn score_in(q: &[String], title: &str, keys: &str, body: &str) -> u32 {
    let (t, k, b) = (tokens(title), tokens(keys), tokens(body));
    let mut score = 0;
    let mut matched = 0;
    for w in q {
        let hit = |list: &[String]| -> u32 {
            let exact = list.iter().filter(|x| *x == w).count() as u32;
            let prefix = if w.chars().count() >= 3 { list.iter().filter(|x| x.starts_with(w.as_str()) && *x != w).count() as u32 } else { 0 };
            exact * 2 + prefix
        };
        let (ht, hk, hb) = (hit(&t), hit(&k), hit(&b));
        if ht + hk + hb == 0 {
            continue;
        }
        matched += 1;
        score += ht * 10 + hk * 6 + hb.min(4);
    }
    // One or two words must all match; a longer question needs half of them.
    let need = if q.len() <= 2 { q.len() } else { q.len().div_ceil(2) };
    if matched < need { 0 } else { score }
}

/// Articles that match `query` (all its words), best first. With an empty
/// query: every article in order. Searches `lang` and English.
pub fn search(query: &str, lang: Lang) -> Vec<Hit> {
    let all = tokens(query);
    let meaningful: Vec<String> = all.iter().filter(|w| !STOPWORDS.contains(&w.as_str())).cloned().collect();
    let q = if meaningful.is_empty() { all } else { meaningful };
    if q.is_empty() {
        return TOPICS.iter().map(|t| Hit { id: t.id, score: 0 }).collect();
    }
    let mut hits: Vec<Hit> = TOPICS
        .iter()
        .filter_map(|t| {
            let mut s = score_in(&q, &title(t.id, lang), &keys(t.id, lang), &body(t.id, lang));
            if lang != Lang::En {
                s = s.max(score_in(&q, &title(t.id, Lang::En), &keys(t.id, Lang::En), &body(t.id, Lang::En)));
            }
            (s > 0).then_some(Hit { id: t.id, score: s })
        })
        .collect();
    hits.sort_by(|a, b| b.score.cmp(&a.score));
    hits
}

/// The `help` tool: the best articles for a question, as JSON the AI can
/// quote. English text (the model translates for the person).
pub fn answer(query: &str, limit: usize) -> Value {
    let hits = search(query, Lang::En);
    let results: Vec<Value> = hits.iter().take(limit.clamp(1, 8)).map(|h| json!({"id": h.id, "title": title(h.id, Lang::En), "text": body(h.id, Lang::En)})).collect();
    if results.is_empty() {
        json!({"results": [], "note": "No help article matches. Try other words, or call `guide` for how the AI tools work."})
    } else {
        json!({"results": results, "topics": TOPICS.iter().map(|t| t.id).collect::<Vec<_>>()})
    }
}

/// The article for a region (F1 with a region focused).
pub fn topic_for_region(r: crate::a11y::Region) -> &'static str {
    match r {
        crate::a11y::Region::Library => "import",
        crate::a11y::Region::Viewer => "start",
        crate::a11y::Region::Inspector => "effects",
        crate::a11y::Region::Timeline => "timeline",
        crate::a11y::Region::Sound | crate::a11y::Region::Record => "audio",
        crate::a11y::Region::Editorial => "timeline",
        crate::a11y::Region::Ai => "ai-editor",
        crate::a11y::Region::Scopes => "colour",
        crate::a11y::Region::Review => "workspaces",
    }
}

// --- the window ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct HelpState {
    query: String,
    open: Option<&'static str>,
    /// Open on this article at the next frame.
    want: Option<&'static str>,
}

fn state_id() -> Id {
    Id::new("help-state")
}

/// Open the help window, on `topic` if given.
pub fn open(app: &mut StudioApp, ctx: &Context, topic: Option<&'static str>) {
    ctx.data_mut(|d| {
        let s = d.get_temp_mut_or_default::<HelpState>(state_id());
        s.want = topic;
        if topic.is_none() {
            s.open = None;
        }
    });
    app.overlay = Overlay::Help;
}

pub fn show(app: &mut StudioApp, ctx: &Context) {
    let lang = crate::i18n::lang();
    let t = theme::tokens(ctx);
    let mut st: HelpState = ctx.data(|d| d.get_temp(state_id())).unwrap_or_default();
    if let Some(w) = st.want.take() {
        st.open = Some(w);
    }
    let hits = search(&st.query, lang);
    let mut close = false;
    let out = kit::modal(ctx, Id::new("help-modal"), Vec2::new(860.0, 580.0), "Help", |ui| {
        ui.add_space(theme::space::SM);
        ui.horizontal(|ui| {
            ui.add_space(theme::space::LG);
            let w = ui.available_width() - 16.0;
            let r = kit::text_field(ui, &mut st.query, "Search help", Some(Icon::MagnifyingGlass), w, None);
            if !r.has_focus() && st.open.is_none() && st.query.is_empty() && ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
        });
        ui.add_space(theme::space::SM);
        let avail = ui.available_rect_before_wrap();
        let list_w = 260.0;
        let rtl = crate::i18n::is_rtl();
        let (list_r, art_r) = {
            let (l, a) = (
                egui::Rect::from_min_size(avail.min + Vec2::new(12.0, 0.0), Vec2::new(list_w, avail.height() - 8.0)),
                egui::Rect::from_min_max(egui::pos2(avail.left() + list_w + 28.0, avail.top()), egui::pos2(avail.right() - 16.0, avail.bottom() - 8.0)),
            );
            if rtl { (crate::i18n::mirror(l, avail), crate::i18n::mirror(a, avail)) } else { (l, a) }
        };
        let mut list = ui.new_child(egui::UiBuilder::new().max_rect(list_r));
        egui::ScrollArea::vertical().id_salt("help-list").auto_shrink([false, false]).show_v(&mut list, |ui| {
            if hits.is_empty() {
                ui.add_space(theme::space::MD);
                ui.label(egui::RichText::new(crate::i18n::shown("No help article matches. Try other words.")).font(theme::body()).color(t.text_2));
            }
            for h in &hits {
                let topic = TOPICS.iter().find(|x| x.id == h.id).unwrap();
                let name = title(h.id, lang);
                if kit::MenuItem::new(&name).icon(topic.icon).checked(st.open == Some(h.id)).show(ui).clicked() {
                    st.open = Some(h.id);
                }
            }
        });
        let mut art = ui.new_child(egui::UiBuilder::new().max_rect(art_r));
        let shown_id = st.open.or(hits.first().map(|h| h.id));
        egui::ScrollArea::vertical().id_salt("help-article").auto_shrink([false, false]).show_v(&mut art, |ui| {
            let Some(id) = shown_id else { return };
            ui.add_space(theme::space::XS);
            let heading = title(id, lang);
            kit::label_a11y(ui, egui::Label::new(egui::RichText::new(crate::i18n::visual(&heading)).font(theme::section_title()).color(t.text)), &heading);
            ui.add_space(theme::space::SM);
            let text = body(id, lang);
            for para in text.split('\n') {
                let font = theme::control();
                let wrapped = kit::wrapped(ui.painter(), para, font.clone(), art_r.width() - 24.0);
                let lab = egui::Label::new(egui::RichText::new(wrapped).font(font).color(t.text));
                kit::label_a11y(ui, if studio_i18n::rtl::has_rtl(para) { lab.extend() } else { lab.wrap() }, para);
                ui.add_space(theme::space::SM);
            }
            ui.add_space(theme::space::SM);
            ui.horizontal_wrapped(|ui| {
                if Button::secondary("Replay the welcome tour").compact().show(ui).clicked() {
                    close = true;
                    ui.ctx().data_mut(|d| d.insert_temp(Id::new("help-replay-tour"), true));
                }
            });
        });
    });
    if out.close || close {
        app.overlay = Overlay::None;
        st.open = None;
        if ctx.data_mut(|d| d.remove_temp::<bool>(Id::new("help-replay-tour"))).unwrap_or(false) {
            app.prefs.tour_done = false;
            app.prefs.tips_seen.clear();
            app.screen = crate::cli::Screen::Editor;
        }
    }
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_split_words_and_cjk() {
        assert_eq!(tokens("Split at the playhead!"), ["split", "at", "the", "playhead"]);
        let t = tokens("剪切片段");
        assert!(t.contains(&"剪".to_string()) && t.contains(&"剪切".to_string()) && t.contains(&"片段".to_string()));
        assert_eq!(tokens("fa-IR 24fps"), ["fa", "ir", "24fps"]);
    }

    #[test]
    fn topics_have_unique_ids() {
        let mut ids: Vec<_> = TOPICS.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), TOPICS.len());
    }

    #[test]
    fn every_article_exists_in_english() {
        for id in message_ids() {
            assert!(studio_i18n::catalogue(Lang::En).contains(&id), "{id} missing from en.ftl");
        }
    }

    #[test]
    fn every_article_exists_in_every_language() {
        for l in Lang::ALL {
            for id in message_ids() {
                assert!(studio_i18n::catalogue(l).contains(&id) || studio_i18n::is_pending(&id), "{id} missing in {}", l.tag());
            }
        }
    }

    #[test]
    fn searching_finds_the_right_article() {
        let hits = search("how do I cut a clip in two", Lang::En);
        assert!(hits.first().is_some_and(|h| h.id == "split-trim"), "{hits:?}");
        assert_eq!(search("export", Lang::En).first().map(|h| h.id), Some("export"));
        assert!(search("zzzzqqq", Lang::En).is_empty());
    }

    #[test]
    fn empty_query_lists_everything() {
        assert_eq!(search("", Lang::En).len(), TOPICS.len());
    }

    #[test]
    fn answers_are_json_the_ai_can_quote() {
        let a = answer("screen reader", 3);
        let r = a["results"].as_array().unwrap();
        assert!(!r.is_empty() && r.len() <= 3);
        assert!(r.iter().any(|x| x["id"] == "accessibility" || x["id"] == "keyboard-nav"), "{a}");
        assert!(r[0]["text"].as_str().unwrap().len() > 40);
        assert!(answer("qqqqzzzz", 3)["results"].as_array().unwrap().is_empty());
    }

    #[test]
    fn the_agent_can_ask_for_help() {
        let tools = crate::agent::mcp::tools();
        let t = tools.iter().find(|t| t["name"] == "howto").expect("the howto tool is listed");
        assert!(t["description"].as_str().unwrap().contains("help"));
        assert_eq!(t["inputSchema"]["required"][0], "query");
    }

    #[test]
    fn other_languages_search_their_own_text_and_english() {
        // English words still find articles when the interface is French.
        let hits = search("export", Lang::Fr);
        assert!(hits.iter().any(|h| h.id == "export"));
    }
}
