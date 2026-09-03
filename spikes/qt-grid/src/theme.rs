use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub struct ThemeLoad {
    pub name: String,
    pub palette: ThemePalette,
    pub signature: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemePalette {
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub muted: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
}

impl Default for ThemePalette {
    fn default() -> Self {
        Self {
            background: "#111318".into(),
            foreground: "#e7e9ee".into(),
            accent: "#7aa2f7".into(),
            muted: "#8d94a3".into(),
            red: "#f7768e".into(),
            green: "#9ece6a".into(),
            yellow: "#e0af68".into(),
            blue: "#7aa2f7".into(),
            magenta: "#bb9af7".into(),
        }
    }
}

pub fn load_active_theme() -> ThemeLoad {
    let Some(path) = active_theme_path() else {
        return fallback();
    };
    let Ok(source) = fs::read_to_string(&path) else {
        return fallback();
    };
    let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let name = canonical
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(theme_title)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Omarchy".into());
    let palette = parse_palette(&source);
    let mut hasher = DefaultHasher::new();
    canonical.hash(&mut hasher);
    source.hash(&mut hasher);
    ThemeLoad {
        name,
        palette,
        signature: hasher.finish(),
    }
}

fn active_theme_path() -> Option<PathBuf> {
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("omarchy/current/theme/colors.toml"))
}

fn fallback() -> ThemeLoad {
    ThemeLoad {
        name: "OmaSheets".into(),
        palette: ThemePalette::default(),
        signature: 0,
    }
}

fn parse_palette(source: &str) -> ThemePalette {
    let mut palette = ThemePalette::default();
    for line in source.lines() {
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let Some(value) = parse_color(raw_value) else {
            continue;
        };
        match key {
            "background" => palette.background = value,
            "foreground" => palette.foreground = value,
            "accent" => palette.accent = value,
            "bright_black" => palette.muted = value,
            "red" => palette.red = value,
            "green" => palette.green = value,
            "yellow" => palette.yellow = value,
            "blue" => palette.blue = value,
            "magenta" => palette.magenta = value,
            _ => {}
        }
    }
    palette
}

fn parse_color(raw: &str) -> Option<String> {
    let value = raw.trim();
    let quote = value.as_bytes().first().copied()?;
    if quote != b'\"' && quote != b'\'' {
        return None;
    }
    let end = value[1..].find(quote as char)? + 1;
    let color = &value[1..end];
    let digits = color.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(color.to_ascii_lowercase())
}

fn theme_title(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normalized_omarchy_colors_with_safe_fallbacks() {
        let palette = parse_palette(
            r#"
                background = "#101216"
                foreground = '#F0F1F2'
                accent = "#ffaa00" # inline comments are allowed
                bright_black = "#667788"
                green = "not-a-color"
                ignored = "#ffffff"
            "#,
        );
        assert_eq!(palette.background, "#101216");
        assert_eq!(palette.foreground, "#f0f1f2");
        assert_eq!(palette.accent, "#ffaa00");
        assert_eq!(palette.muted, "#667788");
        assert_eq!(palette.green, ThemePalette::default().green);
    }

    #[test]
    fn presents_theme_directory_names_for_the_ui() {
        assert_eq!(theme_title("tokyo-night"), "Tokyo Night");
        assert_eq!(theme_title("catppuccin_mocha"), "Catppuccin Mocha");
    }
}
