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
    fn display_preferences_keep_defaults_and_round_trip_through_disk() {
        let previous: Preferences =
            serde_json::from_str(r#"{"appearance":"system","layout":"split","repository":null}"#)
                .unwrap();
        for preferences in [Preferences::default(), previous] {
            assert!(!preferences.tree_file_list);
            assert!(!preferences.show_file_stats);
            assert!(preferences.soft_wrap);
            assert!(preferences.show_commit_message);
        }

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        let mut preferences = Preferences {
            tree_file_list: true,
            show_file_stats: true,
            soft_wrap: false,
            show_commit_message: false,
            language: Language::German,
            ..Preferences::default()
        };
        preferences.remember_repository(PathBuf::from("repository"));
        preferences.write_to(&path).unwrap();
        let restored: Preferences = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(restored.tree_file_list);
        assert!(restored.show_file_stats);
        assert!(!restored.soft_wrap);
        assert!(!restored.show_commit_message);
        assert_eq!(restored.language, Language::German);
        assert_eq!(restored.repository, preferences.repository);
    }

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
    fn repository_sidebar_ratio_round_trips_without_affecting_repository_choice() {
        assert_eq!(Preferences::default().repository_sidebar_ratio, None);
        let preferences = Preferences {
            repository_sidebar_ratio: Some(0.35),
            repository: Some(PathBuf::from("repository")),
            ..Preferences::default()
        };
        let restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&preferences).unwrap()).unwrap();
        assert_eq!(restored.repository_sidebar_ratio, Some(0.35));
        assert_eq!(restored.repository, preferences.repository);
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
        preferences.remember_repository(second.clone());
        assert_eq!(preferences.recent_repositories[0].path, first);
        assert_eq!(preferences.recent_repositories[1].path, second);
        assert_eq!(preferences.repository.as_ref(), Some(&second));
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

    #[test]
    fn repository_names_sort_case_insensitively_with_paths_breaking_ties() {
        let mut preferences = Preferences::default();
        for path in ["z/Zebra", "b/alpha", "a/Alpha", "c/Beta"] {
            preferences.remember_repository(path.into());
        }
        assert_eq!(
            preferences
                .recent_repositories
                .iter()
                .map(|repo| repo.path.clone())
                .collect::<Vec<_>>(),
            ["a/Alpha", "b/alpha", "c/Beta", "z/Zebra"]
                .map(PathBuf::from)
                .to_vec(),
        );
        preferences.remember_repository("z/Zebra".into());
        assert_eq!(
            preferences.recent_repositories[0].path,
            PathBuf::from("a/Alpha")
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

/// The tab the window opens on, which is the last one used.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartView {
    Repositories,
    Compare,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Preferences {
    pub appearance: Appearance,
    pub layout: Layout,
    #[serde(default)]
    pub language: Language,
    pub repository: Option<PathBuf>,
    #[serde(default)]
    pub recent_repositories: Vec<RecentRepository>,
    /// Worktrees opened lately across repositories, newest first.
    #[serde(default)]
    pub recent_worktrees: Vec<PathBuf>,
    #[serde(default)]
    pub repository_sidebar_ratio: Option<f32>,
    #[serde(default)]
    pub tree_file_list: bool,
    #[serde(default)]
    pub show_file_stats: bool,
    #[serde(default = "default_enabled")]
    pub soft_wrap: bool,
    #[serde(default = "default_enabled")]
    pub show_commit_message: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<StartView>,
}

fn default_enabled() -> bool {
    true
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            layout: Layout::default(),
            language: Language::default(),
            repository: None,
            recent_repositories: Vec::new(),
            recent_worktrees: Vec::new(),
            repository_sidebar_ratio: None,
            tree_file_list: false,
            show_file_stats: false,
            soft_wrap: default_enabled(),
            show_commit_message: default_enabled(),
            view: None,
        }
    }
}

const RECENT_WORKTREES: usize = 8;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecentRepository {
    pub path: PathBuf,
    pub viewed_branch: Option<String>,
    /// The linked worktree last opened for this repository; None means the main one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
}

impl Preferences {
    pub fn remember_repository(&mut self, path: PathBuf) {
        if !self
            .recent_repositories
            .iter()
            .any(|entry| entry.path == path)
        {
            self.recent_repositories.push(RecentRepository {
                path: path.clone(),
                viewed_branch: None,
                worktree: None,
            });
        }
        self.sort_repositories();
        self.repository = Some(path);
    }

    fn sort_repositories(&mut self) {
        self.recent_repositories.sort_by_cached_key(|entry| {
            (
                entry
                    .path
                    .file_name()
                    .unwrap_or(entry.path.as_os_str())
                    .to_string_lossy()
                    .to_lowercase(),
                entry.path.clone(),
            )
        });
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

    pub fn remember_worktree(&mut self, path: PathBuf) {
        self.recent_worktrees.retain(|recent| *recent != path);
        self.recent_worktrees.insert(0, path);
        self.recent_worktrees.truncate(RECENT_WORKTREES);
    }

    pub fn worktree(&self, path: &std::path::Path) -> Option<&std::path::Path> {
        self.recent_repositories
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.worktree.as_deref())
    }

    pub fn set_worktree(&mut self, path: &std::path::Path, worktree: Option<PathBuf>) {
        if let Some(entry) = self
            .recent_repositories
            .iter_mut()
            .find(|entry| entry.path == path)
        {
            entry.worktree = worktree;
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
            Ok(bytes) => {
                let mut preferences: Self = serde_json::from_slice(&bytes)
                    .with_context(|| format!("Invalid settings: {}", file.display()))?;
                preferences.sort_repositories();
                Ok(preferences)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("Cannot read {}", file.display())),
        }
    }

    pub fn write(&self) -> Result<()> {
        self.write_to(&Self::file()?)
    }

    pub fn write_to(&self, file: &std::path::Path) -> Result<()> {
        fs::create_dir_all(file.parent().context("Invalid settings directory")?)?;
        let temporary = file.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&temporary, file).context("Cannot save application settings")
    }
}
