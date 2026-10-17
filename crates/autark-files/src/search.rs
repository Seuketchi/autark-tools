//! Search: names in a folder and below, found by code. With `autark-ai` on, a looser set of
//! candidates goes to Jev, which ranks them by meaning (`org.autark.AI1.Rank`); with it off or
//! missing, the code's own ranking stands.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[zbus::proxy(interface = "org.autark.AI1", default_service = "org.autark.AI1", default_path = "/org/autark/AI1")]
trait Ai {
    fn rank(&self, query: &str, items: &[String]) -> zbus::Result<Vec<f64>>;
}

/// Stops a walk through a huge tree.
const MAX_VISITED: usize = 100_000;
const MAX_RESULTS: usize = 300;
/// How many candidates Jev ranks at most, and the score it takes to stay in the results.
const RANKED: usize = 40;
const RELEVANT: f64 = 0.3;

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    /// How well the name matches: higher is better. Candidates only Jev would keep score 0.
    pub score: u32,
}

/// Walks `root` breadth first, so nearer matches come first. Blocks: run it off the main
/// thread. `loose` also collects names matching any word, as candidates for Jev.
pub fn walk(root: &Path, query: &str, hidden: bool, loose: bool, cancelled: &Arc<AtomicBool>) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    let mut queue = VecDeque::from([root.to_path_buf()]);
    let mut visited = 0;
    while let Some(dir) = queue.pop_front() {
        if cancelled.load(Ordering::Relaxed) || visited > MAX_VISITED || hits.len() >= MAX_RESULTS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            visited += 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            if let Some(score) = score(&name, &words) {
                hits.push(Hit { path: path.clone(), score });
            } else if loose && words.iter().any(|word| name.to_lowercase().contains(word.as_str())) {
                hits.push(Hit { path: path.clone(), score: 0 });
            }
            // Links to folders aren't followed: they can loop.
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                queue.push_back(path);
            }
        }
    }
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    hits
}

/// Every word must be in the name; a whole-name match beats a prefix, which beats a word start,
/// which beats anywhere.
fn score(name: &str, words: &[String]) -> Option<u32> {
    let lower = name.to_lowercase();
    let stem = lower.rsplit_once('.').map_or(lower.as_str(), |(stem, _)| stem);
    let mut total = 0;
    for word in words {
        let at = lower.find(word.as_str())?;
        total += if stem == word || lower == *word {
            100
        } else if at == 0 {
            60
        } else if !lower[..at].chars().next_back().is_some_and(char::is_alphanumeric) {
            40
        } else {
            20
        };
    }
    Some(total)
}

/// Reorders `hits` by Jev's scores, dropping candidates it finds unrelated. `None` when
/// `autark-ai` is off or not running, so the caller keeps the code's order.
pub async fn rank(query: &str, root: &Path, hits: &[Hit]) -> Option<Vec<Hit>> {
    let candidates: Vec<&Hit> = hits.iter().take(RANKED).collect();
    let items: Vec<String> = candidates.iter().map(|hit| hit.path.strip_prefix(root).unwrap_or(&hit.path).display().to_string()).collect();
    let connection = zbus::Connection::session().await.ok()?;
    let scores = AiProxy::new(&connection).await.ok()?.rank(query, &items).await.ok()?;
    if scores.len() != candidates.len() {
        return None;
    }
    let mut ranked: Vec<(f64, Hit)> = candidates.into_iter().cloned().zip(scores).map(|(hit, score)| (score, hit)).filter(|(score, hit)| *score >= RELEVANT || hit.score > 0).collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out: Vec<Hit> = ranked.into_iter().map(|(_, hit)| hit).collect();
    // Name matches Jev never saw keep their place after the ranked ones.
    out.extend(hits.iter().skip(RANKED).filter(|hit| hit.score > 0).cloned());
    Some(out)
}

/// Drops the candidates only Jev could have kept.
pub fn named(hits: Vec<Hit>) -> Vec<Hit> {
    hits.into_iter().filter(|hit| hit.score > 0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_rank_whole_names_first() {
        let words = |query: &str| query.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>();
        assert_eq!(score("README.md", &words("readme")), Some(100));
        assert!(score("readme-old.md", &words("readme")) > score("old-readme.md", &words("readme")));
        assert!(score("old-readme.md", &words("readme")) > score("oldreadme.md", &words("readme")));
        assert_eq!(score("notes.md", &words("readme")), None);
        assert_eq!(score("tax 2026 receipts.pdf", &words("receipts 2026")), Some(80));
    }

    #[test]
    fn walks_breadth_first() {
        let root = std::env::temp_dir().join(format!("autark-search-{}", std::process::id()));
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        for file in ["a/b/plan.md", "plan.md", ".git/plan", "a/other.txt"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let hits: Vec<PathBuf> = walk(&root, "plan", false, false, &cancelled).into_iter().map(|hit| hit.path).collect();
        assert_eq!(hits, vec![root.join("plan.md"), root.join("a/b/plan.md")]);
        let loose = walk(&root, "plan other", false, true, &cancelled);
        assert_eq!(loose.iter().filter(|hit| hit.score == 0).count(), 3);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
