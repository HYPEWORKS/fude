//! Finding system fonts for characters the bundled font lacks (CJK, emoji, most scripts).
//!
//! The system font collection is opened only when the first such character turns up, so
//! documents in Latin, Greek and Cyrillic never pay for it.

use fontique::{
    Collection, CollectionOptions, FallbackKey, GenericFamily, Language, QueryFamily, QueryStatus,
    Script, SourceCache,
};

use crate::face::FontData;

/// A font file found for a character.
pub(crate) struct Found {
    pub data: FontData,
    pub index: u32,
    /// Identifies the file and face, so each is loaded once.
    pub key: (u64, u32),
}

pub(crate) struct Fallback {
    collection: Collection,
    sources: SourceCache,
    locale: Option<Language>,
}

impl Fallback {
    pub fn new() -> Fallback {
        Fallback {
            collection: Collection::new(CollectionOptions {
                shared: false,
                system_fonts: true,
            }),
            sources: SourceCache::default(),
            locale: system_locale(),
        }
    }

    /// A font with a glyph for `c`: an emoji font first for emoji, then the system's
    /// monospace fonts, its fallbacks for `c`'s script, and finally sans-serif fonts.
    pub fn find(&mut self, c: char, skip: &[(u64, u32)]) -> Option<Found> {
        let script = script_of(c);
        let emoji = is_emoji(c);
        let mut first: Vec<QueryFamily<'_>> = Vec::new();
        if emoji {
            first.push(QueryFamily::Generic(GenericFamily::Emoji));
        }
        first.push(QueryFamily::Generic(GenericFamily::Monospace));
        let key = FallbackKey::new(script, self.locale.as_ref());
        self.search(c, first, Some(key), skip).or_else(|| {
            self.search(
                c,
                vec![
                    QueryFamily::Generic(GenericFamily::SansSerif),
                    QueryFamily::Generic(GenericFamily::Emoji),
                ],
                None,
                skip,
            )
        })
    }

    fn search(
        &mut self,
        c: char,
        families: Vec<QueryFamily<'_>>,
        fallbacks: Option<FallbackKey>,
        skip: &[(u64, u32)],
    ) -> Option<Found> {
        let mut query = self.collection.query(&mut self.sources);
        query.set_families(families);
        if let Some(key) = fallbacks {
            query.set_fallbacks(key);
        }
        let mut found = None;
        query.matches_with(|font| {
            let covered = font
                .charmap()
                .is_some_and(|map| map.map(c).is_some_and(|g| g != 0));
            let key = (font.blob.id(), font.index);
            if !covered || skip.contains(&key) {
                return QueryStatus::Continue;
            }
            let (data, _) = font.blob.clone().into_raw_parts();
            found = Some(Found {
                data,
                index: font.index,
                key,
            });
            QueryStatus::Stop
        });
        found
    }
}

fn script_of(c: char) -> Script {
    let tag = harfrust::unicode::script(c as u32).tag().to_be_bytes();
    Script::from_bytes(tag)
}

/// Emoji presentation characters, roughly: these should come from a color emoji font.
pub(crate) fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF)
}

/// The user's language, for choosing between regional variants (Japanese vs Chinese
/// forms of the same Han character). From the environment on Unix.
fn system_locale() -> Option<Language> {
    let var = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))?;
    let tag = var.split(['.', '@']).next()?.replace('_', "-");
    if tag == "C" || tag == "POSIX" {
        return None;
    }
    Language::parse(&tag).ok()
}
