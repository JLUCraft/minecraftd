//! Chronological version ordering supplied by a Mojang manifest, including snapshots.
use std::cmp::Ordering;
#[derive(Debug, Clone)]
pub struct VersionOrder {
    versions: Vec<String>,
}
impl VersionOrder {
    #[must_use]
    pub const fn new(versions: Vec<String>) -> Self {
        Self { versions }
    }
    #[must_use]
    pub fn contains(&self, version: &str) -> bool {
        self.versions.iter().any(|entry| entry == version)
    }
    #[must_use]
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        let index = |version: &str| self.versions.iter().position(|entry| entry == version);
        match (index(a), index(b)) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
    #[must_use]
    pub fn major(&self, version: &str) -> Option<String> {
        if let Some(major) = release_major(version) {
            return Some(major);
        }
        let index = self.versions.iter().position(|entry| entry == version)?;
        self.versions[..index]
            .iter()
            .rev()
            .find_map(|entry| release_major(entry))
    }
}
#[must_use]
pub fn release_major(version: &str) -> Option<String> {
    let (head, tail) = version.trim().split_once('.')?;
    if head.is_empty() || !head.bytes().all(|byte| byte.is_ascii_digit()) || tail.is_empty() {
        return None;
    }
    let version = version.split_whitespace().next().unwrap_or(version);
    Some(
        version
            .split('-')
            .next()
            .unwrap_or(version)
            .split('.')
            .take(2)
            .collect::<Vec<_>>()
            .join("."),
    )
}
