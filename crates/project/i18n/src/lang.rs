// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The languages Studio is translated into.

/// A UI language. `En` is the source language and the fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    #[default]
    En,
    Hi,
    Es,
    Fr,
    De,
    Ja,
    ZhHans,
    Ar,
    Fa,
}

impl Lang {
    pub const ALL: [Lang; 9] = [Lang::En, Lang::Hi, Lang::Es, Lang::Fr, Lang::De, Lang::Ja, Lang::ZhHans, Lang::Ar, Lang::Fa];

    /// BCP 47 tag, also the locale file name.
    pub fn tag(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Hi => "hi",
            Lang::Es => "es",
            Lang::Fr => "fr",
            Lang::De => "de",
            Lang::Ja => "ja",
            Lang::ZhHans => "zh-Hans",
            Lang::Ar => "ar",
            Lang::Fa => "fa",
        }
    }

    /// The language's name in itself: what the picker shows.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Hi => "हिन्दी",
            Lang::Es => "Español",
            Lang::Fr => "Français",
            Lang::De => "Deutsch",
            Lang::Ja => "日本語",
            Lang::ZhHans => "简体中文",
            Lang::Ar => "العربية",
            Lang::Fa => "فارسی",
        }
    }

    pub fn english_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Hi => "Hindi",
            Lang::Es => "Spanish",
            Lang::Fr => "French",
            Lang::De => "German",
            Lang::Ja => "Japanese",
            Lang::ZhHans => "Chinese (Simplified)",
            Lang::Ar => "Arabic",
            Lang::Fa => "Persian",
        }
    }

    pub fn is_rtl(self) -> bool {
        matches!(self, Lang::Ar | Lang::Fa)
    }

    /// Match a tag such as `fr`, `fr-CA`, `zh_CN`, `zh-Hans-CN`, `pt`.
    pub fn from_tag(tag: &str) -> Option<Lang> {
        let t = tag.trim().replace('_', "-").to_ascii_lowercase();
        let primary = t.split('-').next().unwrap_or("");
        Some(match primary {
            "en" => Lang::En,
            "hi" => Lang::Hi,
            "es" => Lang::Es,
            "fr" => Lang::Fr,
            "de" => Lang::De,
            "ja" => Lang::Ja,
            "zh" => {
                // Traditional-script locales are not translated yet.
                if t.contains("hant") || t.ends_with("-tw") || t.ends_with("-hk") || t.ends_with("-mo") {
                    return None;
                }
                Lang::ZhHans
            }
            "ar" => Lang::Ar,
            "fa" | "prs" | "pes" => Lang::Fa,
            _ => return None,
        })
    }

    /// The first system language we have a translation for, else English.
    pub fn detect() -> Lang {
        static SYSTEM: std::sync::OnceLock<Lang> = std::sync::OnceLock::new();
        *SYSTEM.get_or_init(|| {
            let env = std::env::var("STUDIO_LANG").ok();
            if let Some(l) = env.as_deref().and_then(Lang::from_tag) {
                return l;
            }
            sys_locale::get_locales().find_map(|t| Lang::from_tag(&t)).unwrap_or(Lang::En)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip() {
        for l in Lang::ALL {
            assert_eq!(Lang::from_tag(l.tag()), Some(l));
        }
    }

    #[test]
    fn regional_tags() {
        assert_eq!(Lang::from_tag("fr-CA"), Some(Lang::Fr));
        assert_eq!(Lang::from_tag("zh_CN"), Some(Lang::ZhHans));
        assert_eq!(Lang::from_tag("zh-Hant-TW"), None);
        assert_eq!(Lang::from_tag("fa-IR"), Some(Lang::Fa));
        assert_eq!(Lang::from_tag("xx"), None);
    }

    #[test]
    fn only_arabic_script_is_rtl() {
        let rtl: Vec<_> = Lang::ALL.iter().filter(|l| l.is_rtl()).collect();
        assert_eq!(rtl, [&Lang::Ar, &Lang::Fa]);
    }
}
