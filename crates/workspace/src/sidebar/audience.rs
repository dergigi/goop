/// NIP-24 is self-declared: People means not marked as a bot, not verified human.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Audience {
    #[default]
    All,
    Bots,
    People,
}

impl Audience {
    pub(super) fn toggle(self, selected: Self) -> Self {
        if self == selected { Self::All } else { selected }
    }

    pub(super) fn matches(self, is_group: bool, is_bot: bool) -> bool {
        match self {
            Self::All => true,
            Self::Bots => !is_group && is_bot,
            Self::People => !is_group && !is_bot,
        }
    }

    pub(super) fn empty_message(self) -> &'static str {
        match self {
            Self::All => "No conversations",
            Self::Bots => "No bot conversations",
            Self::People => "No non-bot conversations",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_are_exclusive_and_toggle_off() {
        let mut filter = Audience::All;
        filter = filter.toggle(Audience::Bots);
        assert_eq!(filter, Audience::Bots);
        filter = filter.toggle(Audience::People);
        assert_eq!(filter, Audience::People);
        filter = filter.toggle(Audience::People);
        assert_eq!(filter, Audience::All);
        assert_eq!(Audience::Bots.toggle(Audience::Bots), Audience::All);
    }

    #[test]
    fn only_unfiltered_list_includes_groups() {
        for is_bot in [false, true] {
            assert!(Audience::All.matches(true, is_bot));
            assert!(!Audience::Bots.matches(true, is_bot));
            assert!(!Audience::People.matches(true, is_bot));
            assert!(Audience::All.matches(false, is_bot));
            assert_eq!(Audience::Bots.matches(false, is_bot), is_bot);
            assert_eq!(Audience::People.matches(false, is_bot), !is_bot);
        }
    }
}
