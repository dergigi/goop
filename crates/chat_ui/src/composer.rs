use nostr_sdk::prelude::PublicKey;

pub(super) fn placeholder(members: &[PublicKey], owner: Option<PublicKey>, name: &str) -> String {
    if owner.is_some_and(|owner| members == [owner]) {
        return "Write a note to your future self".into();
    }
    if let (Some(owner), Ok(goop)) = (owner, PublicKey::parse(state::GOOP_NPUB)) {
        // Room membership includes the sender as well as the recipient.
        if owner != goop && members.contains(&goop)
            && members.iter().all(|member| *member == owner || *member == goop)
        {
            return "Report a bug, suggest a feature, or simply say hi".into();
        }
    }
    format!("Message {name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chat::Room;
    use nostr_sdk::prelude::Keys;

    #[test]
    fn goop_direct_message_uses_custom_placeholder_in_either_member_order() {
        let owner = Keys::generate().public_key();
        let goop = PublicKey::parse(state::GOOP_NPUB).unwrap();
        for room in [Room::new(owner, [goop]), Room::new(goop, [owner])] {
            assert_eq!(placeholder(room.members(), Some(owner), "Goop"),
                "Report a bug, suggest a feature, or simply say hi");
        }
    }

    #[test]
    fn groups_and_similarly_named_people_do_not_use_goop_placeholder() {
        let owner = Keys::generate().public_key();
        let other = Keys::generate().public_key();
        let goop = PublicKey::parse(state::GOOP_NPUB).unwrap();
        let group = Room::new(owner, [goop, other]);
        assert_eq!(placeholder(group.members(), Some(owner), "Group"), "Message Group");
        let person = Room::new(owner, [other]);
        assert_eq!(placeholder(person.members(), Some(owner), "Goop"), "Message Goop");
        assert_eq!(placeholder(&[owner], Some(owner), "Me"), "Write a note to your future self");
        assert_eq!(placeholder(&[goop], Some(goop), "Goop"), "Write a note to your future self");
    }
}

/// Exact, composer-only shortcuts; never claim ordinary movement or selection.
pub(super) fn history_direction(key: &gpui::Keystroke) -> Option<bool> {
    let expected = gpui::Modifiers {
        alt: true,
        platform: cfg!(target_os = "macos"),
        control: !cfg!(target_os = "macos"),
        ..Default::default()
    };
    if key.modifiers != expected { return None; }
    match key.key.as_str() {
        "up" => Some(true),
        "down" => Some(false),
        _ => None,
    }
}

/// A stable snapshot of this chat's sent text, newest first.
pub(super) struct SentHistory {
    entries: Vec<String>,
    index: usize,
    pub draft: String,
}

impl SentHistory {
    pub fn new(entries: Vec<String>, draft: String) -> Option<Self> {
        (!entries.is_empty()).then_some(Self { entries, index: 0, draft })
    }

    pub fn current(&self) -> &str { &self.entries[self.index] }

    /// False means we've moved past the newest message, back to the draft.
    pub fn step(&mut self, older: bool) -> bool {
        if older {
            self.index = (self.index + 1).min(self.entries.len() - 1);
        } else if self.index == 0 {
            return false;
        } else {
            self.index -= 1;
        }
        true
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn navigation_stops_at_oldest_and_returns_to_original_draft() {
        for draft in ["", "unfinished thought"] {
            let mut history = SentHistory::new(vec!["new".into(), "old\nmultiline".into()], draft.into()).unwrap();
            assert_eq!(history.current(), "new");
            assert!(history.step(true));
            assert_eq!(history.current(), "old\nmultiline");
            assert!(history.step(true));
            assert_eq!(history.current(), "old\nmultiline");
            assert!(history.step(false));
            assert_eq!(history.current(), "new");
            assert!(!history.step(false));
            assert_eq!(history.draft, draft);
        }
        assert!(SentHistory::new(vec![], "draft".into()).is_none());
    }

    #[test]
    fn ordinary_cursor_and_selection_shortcuts_are_untouched() {
        for key in ["up", "down", "cmd-up", "cmd-down", "ctrl-up", "ctrl-down",
            "alt-up", "alt-down", "shift-up", "shift-down", "cmd-shift-up",
            "cmd-shift-down", "home", "end"] {
            assert_eq!(history_direction(&gpui::Keystroke::parse(key).unwrap()), None, "{key}");
        }
        let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
        for (key, expected) in [("up", true), ("down", false)] {
            assert_eq!(history_direction(&gpui::Keystroke::parse(&format!("{modifier}-alt-{key}")).unwrap()), Some(expected));
            assert_eq!(history_direction(&gpui::Keystroke::parse(&format!("{modifier}-alt-shift-{key}")).unwrap()), None);
        }
    }
}
