use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::i18n;

/// UI language: an index into the bundled locales, English being the first.
/// English text is the source; the other languages translate it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Language(pub(crate) usize);

impl Language {
    pub const ENGLISH: Self = Self(0);

    /// Every language the app ships, English first.
    pub fn all() -> Vec<Self> {
        (0..i18n::locales().len()).map(Self).collect()
    }

    /// The locale code, such as `zh-CN`; saved in the app state.
    pub fn code(self) -> &'static str {
        &i18n::locales()[self.0].code
    }

    pub fn from_code(code: &str) -> Option<Self> {
        // Codes saved before languages moved to locale files.
        let code = match code {
            "English" => "en",
            "SimplifiedChinese" => "zh-CN",
            code => code,
        };
        i18n::locales()
            .iter()
            .position(|locale| locale.code.eq_ignore_ascii_case(code))
            .map(Self)
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Each language is shown in its own script so it can be found by anyone.
        f.write_str(&i18n::locales()[self.0].name)
    }
}

impl Serialize for Language {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.code().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Language {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        // A language no longer bundled falls back to English instead of
        // discarding the whole saved state.
        Ok(Self::from_code(&code).unwrap_or_default())
    }
}
