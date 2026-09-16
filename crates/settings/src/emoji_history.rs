use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const DEFAULTS: [&str; 3] = ["🤙", "🧡", "👀"];
const RECENT_LIMIT: usize = 40;
const REACTION_WINDOW: usize = 100;

/// Local picker history and a bounded window of reactions sent by this user.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmojiHistory {
    pub recent: Vec<String>,
    reactions: Vec<String>,
}

impl EmojiHistory {
    pub fn record(&mut self, emoji: &str, reaction: bool) {
        self.recent.retain(|e| e != emoji);
        self.recent.insert(0, emoji.to_owned());
        self.recent.truncate(RECENT_LIMIT);
        if reaction {
            self.reactions.insert(0, emoji.to_owned());
            self.reactions.truncate(REACTION_WINDOW);
        }
    }

    pub fn quick_reactions(&self) -> Vec<String> {
        let mut counts: HashMap<&str, (usize, usize)> = HashMap::new();
        for (index, emoji) in self.reactions.iter().take(REACTION_WINDOW).enumerate() {
            let entry = counts.entry(emoji).or_insert((0, index));
            entry.0 += 1;
        }
        let mut ranked: Vec<_> = counts.into_iter().collect();
        ranked.sort_by_key(|(_, (count, recent))| (std::cmp::Reverse(*count), *recent));
        let mut result: Vec<String> = ranked
            .into_iter()
            .take(3)
            .map(|(e, _)| e.to_owned())
            .collect();
        for emoji in DEFAULTS {
            if result.len() == 3 {
                break;
            }
            if !result.iter().any(|e| e == emoji) {
                result.push(emoji.to_owned());
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_composer_history_do_not_change_quick_reactions() {
        let mut history = EmojiHistory::default();
        history.record("🚀", false);
        history.record("🧡", false);
        history.record("🚀", false);
        assert_eq!(history.recent, ["🚀", "🧡"]);
        assert_eq!(history.quick_reactions(), DEFAULTS);
    }
    #[test]
    fn frequency_wins_and_recency_breaks_ties() {
        let mut h = EmojiHistory::default();
        for e in ["🤙", "🤙", "🚀", "🧡"] {
            h.record(e, true);
        }
        assert_eq!(h.quick_reactions(), ["🤙", "🧡", "🚀"]);
        for _ in 0..100 {
            h.record("👀", true);
        }
        assert_eq!(h.quick_reactions(), ["👀", "🤙", "🧡"]);
    }
    #[test]
    fn saved_history_preserves_variants_and_ranking() {
        let mut h = EmojiHistory::default();
        h.record("🤙🏽", true);
        h.record("👩🏿‍💻", false);
        let loaded: EmojiHistory =
            serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(loaded.recent, h.recent);
        assert_eq!(loaded.quick_reactions(), ["🤙🏽", "🤙", "🧡"]);
        for i in 0..50 {
            h.record(&i.to_string(), false);
        }
        assert_eq!(h.recent.len(), RECENT_LIMIT);
    }
}
