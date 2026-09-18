/// One active chat-list filter. People means not self-declared as a bot (NIP-24).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ChatFilter {
    #[default]
    All,
    Bots,
    People,
    Drafts,
}

impl ChatFilter {
    pub(super) fn toggle(self, selected: Self) -> Self {
        if self == selected { Self::All } else { selected }
    }

    pub(super) fn matches(self, is_group: bool, is_bot: bool, has_draft: bool) -> bool {
        match self {
            Self::All => true,
            Self::Bots => !is_group && is_bot,
            Self::People => !is_group && !is_bot,
            Self::Drafts => has_draft,
        }
    }

    pub(super) fn empty_message(self) -> &'static str {
        match self {
            Self::All => "No conversations",
            Self::Bots => "No bot conversations",
            Self::People => "No non-bot conversations",
            Self::Drafts => "No drafts",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_are_exclusive_and_toggle_off() {
        let mut filter = ChatFilter::All;
        filter = filter.toggle(ChatFilter::Bots);
        assert_eq!(filter, ChatFilter::Bots);
        filter = filter.toggle(ChatFilter::People);
        assert_eq!(filter, ChatFilter::People);
        filter = filter.toggle(ChatFilter::People);
        assert_eq!(filter, ChatFilter::All);
        assert_eq!(ChatFilter::Bots.toggle(ChatFilter::Bots), ChatFilter::All);
    }

    #[test]
    fn draft_filter_includes_groups_and_is_exclusive_with_audience_filters() {
        for is_group in [false, true] {
            for is_bot in [false, true] {
                assert!(ChatFilter::Drafts.matches(is_group, is_bot, true));
                assert!(!ChatFilter::Drafts.matches(is_group, is_bot, false));
            }
        }
        assert_eq!(ChatFilter::Bots.toggle(ChatFilter::Drafts), ChatFilter::Drafts);
        assert_eq!(ChatFilter::Drafts.toggle(ChatFilter::People), ChatFilter::People);
        assert_eq!(ChatFilter::Drafts.toggle(ChatFilter::Drafts), ChatFilter::All);
    }

    #[test]
    fn audience_filters_exclude_groups() {
        for is_bot in [false, true] {
            assert!(ChatFilter::All.matches(true, is_bot, false));
            assert!(!ChatFilter::Bots.matches(true, is_bot, false));
            assert!(!ChatFilter::People.matches(true, is_bot, false));
            assert!(ChatFilter::All.matches(false, is_bot, false));
            assert_eq!(ChatFilter::Bots.matches(false, is_bot, false), is_bot);
            assert_eq!(ChatFilter::People.matches(false, is_bot, false), !is_bot);
        }
    }
}
