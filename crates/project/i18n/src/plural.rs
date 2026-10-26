// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CLDR cardinal plural categories for whole numbers.

use crate::Lang;

/// The category name a selector variant is written with.
pub fn category(lang: Lang, n: i64) -> &'static str {
    let n = n.unsigned_abs();
    match lang {
        Lang::Ja | Lang::ZhHans => "other",
        Lang::En | Lang::De => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
        // Spanish: exact millions are "many" (CLDR).
        Lang::Es => match n {
            1 => "one",
            n if n != 0 && n % 1_000_000 == 0 => "many",
            _ => "other",
        },
        // French and Hindi: 0 and 1 are singular. (French also has "many"
        // for exact millions; the messages never need it.)
        Lang::Fr => match n {
            0 | 1 => "one",
            n if n % 1_000_000 == 0 => "many",
            _ => "other",
        },
        Lang::Hi | Lang::Fa => {
            if n <= 1 {
                "one"
            } else {
                "other"
            }
        }
        Lang::Ar => match n {
            0 => "zero",
            1 => "one",
            2 => "two",
            _ => match n % 100 {
                3..=10 => "few",
                11..=99 => "many",
                _ => "other",
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english() {
        assert_eq!(category(Lang::En, 1), "one");
        assert_eq!(category(Lang::En, 0), "other");
        assert_eq!(category(Lang::En, 2), "other");
    }

    #[test]
    fn french_zero_is_singular() {
        assert_eq!(category(Lang::Fr, 0), "one");
        assert_eq!(category(Lang::Fr, 2), "other");
    }

    #[test]
    fn arabic_has_six() {
        let got: Vec<_> = [0, 1, 2, 3, 10, 11, 99, 100, 102].iter().map(|&n| category(Lang::Ar, n)).collect();
        assert_eq!(got, ["zero", "one", "two", "few", "few", "many", "many", "other", "other"]);
    }

    #[test]
    fn cjk_never_varies() {
        assert_eq!(category(Lang::Ja, 1), "other");
        assert_eq!(category(Lang::ZhHans, 5), "other");
    }
}

#[cfg(test)]
mod million_tests {
    use super::*;

    #[test]
    fn exact_millions_are_many_in_french_and_spanish() {
        for l in [Lang::Fr, Lang::Es] {
            assert_eq!(category(l, 999_999), "other");
            assert_eq!(category(l, 1_000_000), "many");
            assert_eq!(category(l, 2_000_000), "many");
            assert_eq!(category(l, -3_000_000), "many");
        }
        assert_eq!(category(Lang::En, 1_000_000), "other");
    }
}
