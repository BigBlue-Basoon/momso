use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BrowserChoice {
    #[default]
    Auto,
    Chrome,
    Whale,
    Edge,
}

impl BrowserChoice {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Chrome => "chrome",
            Self::Whale => "whale",
            Self::Edge => "edge",
        }
    }
}

fn locations(kind: BrowserChoice) -> Vec<PathBuf> {
    let relative = match kind {
        BrowserChoice::Chrome => r"Google\Chrome\Application\chrome.exe",
        BrowserChoice::Whale => r"Naver\Naver Whale\Application\whale.exe",
        BrowserChoice::Edge => r"Microsoft\Edge\Application\msedge.exe",
        BrowserChoice::Auto => return Vec::new(),
    };
    ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|base| PathBuf::from(base).join(relative))
        .collect()
}

pub(crate) fn resolve(choice: BrowserChoice) -> Option<(BrowserChoice, PathBuf)> {
    let order: &[BrowserChoice] = match choice {
        BrowserChoice::Auto => &[
            BrowserChoice::Chrome,
            BrowserChoice::Whale,
            BrowserChoice::Edge,
        ],
        BrowserChoice::Chrome => &[BrowserChoice::Chrome],
        BrowserChoice::Whale => &[BrowserChoice::Whale],
        BrowserChoice::Edge => &[BrowserChoice::Edge],
    };
    order.iter().find_map(|kind| {
        locations(*kind)
            .into_iter()
            .filter(|path| path.is_file())
            .find_map(|path| {
                let path = if *kind == BrowserChoice::Whale {
                    whale_binary(&path)?
                } else {
                    path
                };
                Some((*kind, path))
            })
    })
}

fn whale_binary(launcher: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<_> = std::fs::read_dir(launcher.parent()?)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let numbers: Vec<u32> = name
                .split('.')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            if numbers.len() < 2 {
                return None;
            }
            let executable = entry.path().join("whale.exe");
            executable.is_file().then_some((numbers, executable))
        })
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.into_iter().next().map(|(_, path)| path)
}

pub(crate) fn read(root: &Path) -> Result<BrowserChoice, &'static str> {
    match std::fs::read(root.join("browser-choice.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "unavailable"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BrowserChoice::Auto),
        Err(_) => Err("unavailable"),
    }
}

pub(crate) fn write(root: &Path, choice: BrowserChoice) -> Result<(), &'static str> {
    std::fs::create_dir_all(root).map_err(|_| "unavailable")?;
    let value = serde_json::to_vec(&choice).map_err(|_| "unavailable")?;
    std::fs::write(root.join("browser-choice.json"), value).map_err(|_| "unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_choice_round_trips_and_rejects_unknown_values() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(read(root.path()), Ok(BrowserChoice::Auto));
        for choice in [
            BrowserChoice::Auto,
            BrowserChoice::Chrome,
            BrowserChoice::Whale,
            BrowserChoice::Edge,
        ] {
            write(root.path(), choice).unwrap();
            assert_eq!(read(root.path()), Ok(choice));
        }
        assert!(serde_json::from_str::<BrowserChoice>("\"other\"").is_err());
    }

    #[test]
    fn whale_selects_newest_versioned_binary_not_launcher() {
        let root = tempfile::tempdir().unwrap();
        let launcher = root.path().join("whale.exe");
        std::fs::write(&launcher, b"").unwrap();
        for version in ["5.9.1.0", "5.41.1.0"] {
            let folder = root.path().join(version);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("whale.exe"), b"").unwrap();
        }
        assert_eq!(
            whale_binary(&launcher),
            Some(root.path().join("5.41.1.0/whale.exe"))
        );
    }
}
