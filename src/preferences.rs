use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_language_uses_preference_order_regions_and_chinese_scripts() {
        for (locale, expected) in [
            ("en-GB", Language::English),
            ("zh_CN.UTF-8", Language::SimplifiedChinese),
            ("zh-SG", Language::SimplifiedChinese),
            ("zh-TW", Language::TraditionalChinese),
            ("zh-HK", Language::TraditionalChinese),
            ("zh-MO", Language::TraditionalChinese),
            ("zh-Hans-HK", Language::SimplifiedChinese),
            ("zh-Hant-CN", Language::TraditionalChinese),
            ("JA-jp", Language::Japanese),
            ("ko-KR", Language::Korean),
            ("fr-CA", Language::French),
            ("de-DE", Language::German),
            ("es-MX", Language::Spanish),
            ("pt_BR.UTF-8", Language::BrazilianPortuguese),
        ] {
            assert_eq!(Language::from_locale(locale), Some(expected), "{locale}");
        }
        assert_eq!(
            Language::System.resolve(["it-IT", "ja-JP", "en-US"]),
            Language::Japanese
        );
        assert_eq!(Language::System.resolve(["C", "ru-RU"]), Language::English);
        assert_eq!(
            Language::System.resolve(std::iter::empty::<&str>()),
            Language::English
        );
        assert_eq!(Language::German.resolve(["zh-CN"]), Language::German);
    }

    #[test]
    fn language_choice_round_trips_without_changing_other_preferences() {
        for language in Language::ALL {
            let preferences = Preferences {
                language,
                appearance: Appearance::Dark,
                layout: Layout::Inline,
                ..Preferences::default()
            };
            let restored: Preferences =
                serde_json::from_slice(&serde_json::to_vec(&preferences).unwrap()).unwrap();
            assert_eq!(restored.language, language);
            assert_eq!(restored.appearance, Appearance::Dark);
            assert_eq!(restored.layout, Layout::Inline);
        }
        assert_eq!(Preferences::default().language, Language::System);
    }

    #[test]
    fn recent_repositories_deduplicate_and_keep_independent_viewed_branches() {
        let mut preferences = Preferences::default();
        let first = PathBuf::from("first");
        let second = PathBuf::from("second");
        preferences.remember_repository(first.clone());
        preferences.set_viewed_branch(&first, Some("refs/heads/feature".into()));
        preferences.remember_repository(second.clone());
        preferences.set_viewed_branch(&second, Some("refs/remotes/origin/main".into()));
        preferences.remember_repository(first.clone());
        assert_eq!(preferences.recent_repositories.len(), 2);
        assert_eq!(preferences.recent_repositories[0].path, first);
        assert_eq!(preferences.repository.as_ref(), Some(&first));
        let restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&preferences).unwrap()).unwrap();
        assert_eq!(restored.viewed_branch(&first), Some("refs/heads/feature"));
        assert_eq!(
            restored.viewed_branch(&second),
            Some("refs/remotes/origin/main")
        );
        preferences.set_viewed_branch(&first, None);
        assert_eq!(preferences.viewed_branch(&first), None);
        assert_eq!(
            preferences.viewed_branch(&second),
            Some("refs/remotes/origin/main")
        );
    }
}

impl Appearance {
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    Split,
    Inline,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Language {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh-CN")]
    SimplifiedChinese,
    #[serde(rename = "zh-TW")]
    TraditionalChinese,
    #[serde(rename = "ja")]
    Japanese,
    #[serde(rename = "ko")]
    Korean,
    #[serde(rename = "fr")]
    French,
    #[serde(rename = "de")]
    German,
    #[serde(rename = "es")]
    Spanish,
    #[serde(rename = "pt-BR")]
    BrazilianPortuguese,
}

impl Language {
    pub const ALL: [Self; 10] = [
        Self::System,
        Self::English,
        Self::SimplifiedChinese,
        Self::TraditionalChinese,
        Self::Japanese,
        Self::Korean,
        Self::French,
        Self::German,
        Self::Spanish,
        Self::BrazilianPortuguese,
    ];

    pub fn code(self) -> Option<&'static str> {
        Some(match self {
            Self::System => return None,
            Self::English => "en",
            Self::SimplifiedChinese => "zh-CN",
            Self::TraditionalChinese => "zh-TW",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::French => "fr",
            Self::German => "de",
            Self::Spanish => "es",
            Self::BrazilianPortuguese => "pt-BR",
        })
    }

    pub fn from_locale(locale: &str) -> Option<Self> {
        let normalized = locale.replace('_', "-").to_ascii_lowercase();
        let locale = normalized.split(['.', '@']).next()?;
        let parts: Vec<_> = locale.split('-').collect();
        Some(match *parts.first()? {
            "en" => Self::English,
            "zh" => {
                if parts.contains(&"hant") {
                    Self::TraditionalChinese
                } else if parts.contains(&"hans") {
                    Self::SimplifiedChinese
                } else if parts.iter().any(|part| ["tw", "hk", "mo"].contains(part)) {
                    Self::TraditionalChinese
                } else {
                    Self::SimplifiedChinese
                }
            }
            "ja" => Self::Japanese,
            "ko" => Self::Korean,
            "fr" => Self::French,
            "de" => Self::German,
            "es" => Self::Spanish,
            "pt" => Self::BrazilianPortuguese,
            _ => return None,
        })
    }

    pub fn resolve(self, locales: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        if self != Self::System {
            return self;
        }
        locales
            .into_iter()
            .find_map(|locale| Self::from_locale(locale.as_ref()))
            .unwrap_or(Self::English)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Preferences {
    pub appearance: Appearance,
    pub layout: Layout,
    #[serde(default)]
    pub language: Language,
    pub repository: Option<PathBuf>,
    #[serde(default)]
    pub recent_repositories: Vec<RecentRepository>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecentRepository {
    pub path: PathBuf,
    pub viewed_branch: Option<String>,
}

impl Preferences {
    pub fn remember_repository(&mut self, path: PathBuf) {
        let entry = self
            .recent_repositories
            .iter()
            .position(|entry| entry.path == path)
            .map(|index| self.recent_repositories.remove(index))
            .unwrap_or_else(|| RecentRepository {
                path: path.clone(),
                viewed_branch: None,
            });
        self.recent_repositories.insert(0, entry);
        self.repository = Some(path);
    }

    pub fn viewed_branch(&self, path: &std::path::Path) -> Option<&str> {
        self.recent_repositories
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.viewed_branch.as_deref())
    }

    pub fn set_viewed_branch(&mut self, path: &std::path::Path, reference: Option<String>) {
        if let Some(entry) = self
            .recent_repositories
            .iter_mut()
            .find(|entry| entry.path == path)
        {
            entry.viewed_branch = reference;
        }
    }

    fn file() -> Result<PathBuf> {
        Ok(ProjectDirs::from("", "", env!("CARGO_PKG_NAME"))
            .context("Cannot locate application settings directory")?
            .config_dir()
            .join("preferences.json"))
    }

    pub fn read() -> Result<Self> {
        let file = Self::file()?;
        match fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("Invalid settings: {}", file.display())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("Cannot read {}", file.display())),
        }
    }

    pub fn write(&self) -> Result<()> {
        let file = Self::file()?;
        fs::create_dir_all(file.parent().context("Invalid settings directory")?)?;
        let temporary = file.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&temporary, &file).context("Cannot save application settings")
    }
}
