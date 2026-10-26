# Localisation

Zornov's interface is available in English, Hindi, Spanish, French, German,
Japanese, Simplified Chinese, Arabic and Persian. Arabic and Persian lay the
editor out right to left. Pick the language in **Settings ▸ General ▸
Language**; "System default" follows the computer.

The code is the `studio-i18n` crate (`crates/project/i18n`) and `app/src/i18n.rs`.

## If you are adding interface text (read this one)

1. Write the English in the code as you always did: `Button::primary("Export")`,
   `kit::tip(resp, "Rename project", None)`, `RichText::new("Not saved")`. The
   shared widgets in `kit` translate whatever text they are given.
2. Run

   ```
   cargo run -p studio-i18n --bin i18n-scan -- --add
   ```

   It finds interface text with no message, adds `id = English` to
   `crates/project/i18n/locales/en.ftl` and lists the id in `locales/pending.txt`.
   Until a translator reaches it, every other language shows the English.
3. `cargo test -p studio-i18n` checks that you did (see below).

For text that is built at run time, wrap the `format!` in `crate::i18n::tf`:
`kit::push(ctx, crate::i18n::tf(format!("Undid {}", what)), None, None)`. The
catalogue holds `undid = Undid { $v1 }`; the translation puts the pieces in
the language's order and translates the words it knows.

For counts and anything with a plural, use a message with a selector and
`tr_args`, because "1 clip" / "2 clips" differs per language:

```
clips-selected = { $n ->
    [one] { $n } clip selected
   *[other] { $n } clips selected
 }
```

```rust
let text = crate::i18n::count("clips-selected", n);
```

Anything handed straight to egui's painter or to `RichText` (not through a
`kit` widget) must go through `crate::i18n::shown("…")`, which translates and,
for right-to-left text, shapes and reorders it. Text a person typed or the AI
wrote (project names, clip names) is not translated, but `shown` still shapes
it when it is Arabic or Persian.

## The message format

`locales/<tag>.ftl`, a Fluent-style subset (`crates/project/i18n/src/ftl.rs`):

```
# comment
export = Export
undid = Undid { $v1 }
hello = Hello, { $name }. { app-name } is ready.
brace = Use { "{" } and { "}" } for literal braces.
long =
    first paragraph
    second paragraph
```

* `{ $name }` a variable; `{ other-id }` another message; `{ "text" }` a literal.
* `{ $n -> [one] … *[other] … }` a selector. Keys are CLDR plural categories
  (`zero one two few many other`), exact numbers (`[0]`) or strings
  (`[yes]`). One variant is the default, marked `*`.
* Numbers in `Value::Num` are written with the language's digits (Persian,
  Arabic).
* Terms, attributes and functions are not supported.

Message ids are lower-case words joined by hyphens. Ids made by `--add` are a
slug of the English text; give a message a better id when you write it by hand
(`tour-library-title`).

## Tests

`crates/project/i18n/tests/catalogue.rs`: every file parses; no message defined twice;
every language has every message (or the id is in `pending.txt`) with the same
variables as English; plural keys belong to the language; nothing in the
Arabic and Persian files is still English.

`crates/project/i18n/tests/sources.rs`: **hard-coded text is caught here.** A scanner
reads the UI sources (`kit`, `screens`, `editor`, `app.rs`, …) and finds string
literals passed to interface calls (button labels, tooltips, headings,
`label:` fields…). Each must have a message. `tests/hardcoded_baseline.txt`
lists the few that are not text for people; it may only shrink.

`app/src/ui_tests.rs`: the real app runs headless (no window) in every
language, and the tests check what is drawn, what a screen reader is told, the
right-to-left layout, the keyboard commands, the tour, tips, help and
workspaces.

## Translating

`i18n-scan --todo fa` prints the English messages Persian lacks, numbered.
Write `N<TAB>translation` lines and add them with
`i18n-scan --merge fa file.tsv`; each line is checked (parses, same
variables, plural categories valid for the language) and rejected with the
reason if not. Remove the ids from `pending.txt` once every language has them
(`i18n-scan --pending` lists those).

The first translations of all languages were produced with an AI model from a
glossary of editing terms (`docs/translation-brief.md`), checked by script, and
reviewed by a second pass; a native reader should still read them.
Corrections are plain edits of the `.ftl` files.

## Adding a language

1. Add the variant to `Lang` (`src/lang.rs`): tag, native name, whether it is
   right to left, and the plural rule in `src/plural.rs`.
2. Add `locales/<tag>.ftl` and its line in `SOURCES` (`src/lib.rs`).
3. Fill it with `--todo` / `--merge`.

## Right-to-left text

egui (0.37) shapes each word with HarfBuzz, so Arabic and Persian letters join,
ligatures (لا) form and marks attach, Devanagari conjuncts and vowel signs are
built, and the letters of an Arabic word come out right to left. What egui does
not do is order the *words* of a line: they are laid out in the order they are
written, left to right. `studio_i18n::rtl::display` fixes that with the Unicode
bidirectional algorithm applied to whole words: it returns the same words in
visual order (letters inside a word stay in logical order, so shaping is
untouched), keeps numbers and Latin names reading left to right, and swaps
brackets that end up in a right-to-left run. Wrapped paragraphs are broken in
reading order first (`kit::wrapped`) because wrapping reordered text breaks
lines at the wrong end.

The reordered string is for drawing only. Widgets give a screen reader the
logical text (`WidgetInfo`, `kit::label_a11y`).

In right-to-left languages the panels mirror (Library on the right, Inspector
on the left), menus and sections start at the right edge, and the timeline
keeps running left to right as time does. The fonts for Arabic, Persian,
Devanagari, Japanese and Chinese come from the operating system (egui looks
them up on demand); Zornov bundles none of them.

### Known limits

* Text typed into a text field is laid out by egui's text editor, which draws
  what was typed in the order it was typed: Arabic and Persian words are joined
  and right to left each, but a sentence with several words reads from its first
  word on the left while editing. Titles on the timeline are drawn by the
  renderer (`studio-text`), which does full bidirectional layout.
* A plain `egui::Label` built from `shown(...)` text exposes the reordered words
  to a screen reader in the right-to-left languages; widgets that matter (kit
  controls, help, tour, tips) give the logical text.
* Dates, file sizes and times use ASCII digits except where a message goes
  through `tr_args` with a number.
* Text in the AI editor's replies and anything an AI wrote is in the language
  the AI chose, not translated by Zornov.
