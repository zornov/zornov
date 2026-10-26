// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Writes every built-in template to `templates/<id>.json`.
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    std::fs::create_dir_all(&dir).unwrap();
    for s in studio_text::templates::all() {
        std::fs::write(dir.join(format!("{}.json", s.id)), s.to_json()).unwrap();
    }
    println!("wrote {} templates to {}", studio_text::templates::all().len(), dir.display());
}
