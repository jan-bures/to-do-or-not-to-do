//! Bounded, non-authoritative discovery from GitHub's public Atom release history.

use super::release::package_version;
use semver::Version;

pub(super) const MAX_HISTORY: u64 = 1024 * 1024;
const MAX_CANDIDATES: usize = 5;
const ATOM: &str = "http://www.w3.org/2005/Atom";
const RELEASE_ID: &str = "tag:github.com,2008:Repository/";

pub(super) fn candidates(xml: &str) -> Option<Vec<Version>> {
    // roxmltree rejects DTDs by default. Only direct Atom entry IDs are meaningful; never follow
    // an XML URL or let markup in the release notes influence discovery.
    if xml.len() as u64 > MAX_HISTORY || xml.contains("<!DOCTYPE") {
        return None;
    }
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if !root.has_tag_name((ATOM, "feed")) {
        return None;
    }
    let mut releases = Vec::new();
    for entry in root.children().filter(|n| n.has_tag_name((ATOM, "entry"))) {
        let mut ids = entry.children().filter(|n| n.has_tag_name((ATOM, "id")));
        let id = ids.next()?;
        if ids.next().is_some() || id.children().any(|n| n.is_element()) {
            return None;
        }
        let text: String = id
            .children()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect();
        if text.trim().is_empty() {
            return None;
        }
        let Some((repository, tag)) = text
            .trim()
            .strip_prefix(RELEASE_ID)
            .and_then(|s| s.split_once('/'))
        else {
            continue;
        };
        if repository.is_empty() || !repository.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Some((version, height)) = tag.strip_prefix('v').and_then(package_version) {
            releases.push((height, version));
        }
    }
    releases.sort_unstable_by(|a, b| b.cmp(a));
    releases.dedup();
    Some(
        releases
            .into_iter()
            .take(MAX_CANDIDATES)
            .map(|(_, version)| version)
            .collect(),
    )
}

#[cfg(test)]
pub(super) fn atom(tags: &[&str]) -> String {
    let entries: String = tags
        .iter()
        .map(|tag| format!("<entry><id>{RELEASE_ID}123/{tag}</id></entry>"))
        .collect();
    format!("<feed xmlns=\"{ATOM}\">{entries}</feed>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_builds_are_unique_sorted_and_capped_at_five() {
        let xml = atom(&[
            "v0.2.0-build.8",
            "v0.1.0-build.12",
            "v0.1.0-build.9",
            "beta",
            "v0.1.0-build.11",
            "v0.1.0-build.10",
            "v0.1.0-build.10",
            "v0.1.0-build.7",
            "v0.1.0-build.0",
        ]);
        let versions: Vec<_> = candidates(&xml)
            .expect("valid Atom")
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            versions,
            [
                "0.1.0-build.12",
                "0.1.0-build.11",
                "0.1.0-build.10",
                "0.1.0-build.9",
                "0.2.0-build.8"
            ]
        );
    }

    #[test]
    fn invalid_atom_and_oversized_bodies_are_rejected() {
        for xml in [
            "<feed>",
            "<feed/>",
            "<!DOCTYPE feed> <feed/>",
            "<feed xmlns=\"http://www.w3.org/2005/Atom\"><entry/></feed>",
        ] {
            assert!(candidates(xml).is_none());
        }
        assert!(candidates(&" ".repeat(MAX_HISTORY as usize + 1)).is_none());
    }
}
