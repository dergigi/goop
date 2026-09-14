//! Find within one chat's decrypted messages. No relay requests are made here.
use super::*;

pub(super) struct FindBar {
    pub input: Entity<InputState>,
    pub open: bool,
    pub owner: WeakEntity<ChatPanel>,
    pub pattern: Option<Regex>,
    pub dirty: bool,
    matches: Vec<EventId>,
    active: Option<EventId>,
    return_focus: Option<FocusHandle>,
}

impl FindBar {
    pub fn new(input: Entity<InputState>, owner: WeakEntity<ChatPanel>) -> Self {
        Self {
            input,
            owner,
            pattern: None,
            open: false,
            dirty: false,
            matches: Vec::new(),
            active: None,
            return_focus: None,
        }
    }

    pub fn active(&self, id: EventId) -> bool {
        self.open && self.active == Some(id)
    }
}

fn search_pattern(query: &str) -> Option<Regex> {
    if query.is_empty() {
        return None;
    }
    regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
        .ok()
}

fn matching_messages<'a>(
    query: &str,
    messages: impl Iterator<Item = (EventId, &'a str)>,
) -> Vec<EventId> {
    if query.is_empty() {
        return Vec::new();
    }
    let Some(pattern) = search_pattern(query) else {
        return Vec::new();
    };
    messages
        .filter_map(|(id, text)| pattern.is_match(text).then_some(id))
        .collect()
}

fn next_match(matches: &[EventId], active: Option<EventId>, previous: bool) -> Option<EventId> {
    if matches.is_empty() {
        return None;
    }
    let index = active.and_then(|id| matches.iter().position(|candidate| *candidate == id));
    let next = match (index, previous) {
        (Some(index), true) => (index + matches.len() - 1) % matches.len(),
        (Some(index), false) => (index + 1) % matches.len(),
        (None, true) => matches.len() - 1,
        (None, false) => 0,
    };
    Some(matches[next])
}

impl ChatPanel {
    pub fn focus_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .find
            .input
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            self.find.return_focus = window.focused(cx);
        }
        self.find.open = true;
        self.refresh_find(false, cx);
        self.find.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(&ui::input::SelectAll, window, cx);
        });
        cx.notify();
    }

    pub(super) fn refresh_find(&mut self, reset: bool, cx: &mut Context<Self>) {
        self.find.dirty = false;
        if !self.find.open {
            return;
        }
        let people: HashMap<_, _> = PersonRegistry::global(cx)
            .read(cx)
            .loaded(cx)
            .into_iter()
            .map(|person| (person.public_key(), person.name()))
            .collect();
        for message in &self.messages {
            self.rendered_texts_by_id
                .entry(message.id)
                .or_insert_with(|| {
                    RenderedText::render(
                        &message.content,
                        &message.mentions,
                        self.render_markdown,
                        |mention| {
                            format!(
                                "@{}",
                                people
                                    .get(&mention.public_key)
                                    .cloned()
                                    .unwrap_or_else(|| Person::from(mention.public_key).name())
                            )
                        },
                    )
                });
        }
        let query = self.find.input.read(cx).value();
        self.find.pattern = search_pattern(&query);
        self.find.matches = matching_messages(
            &query,
            self.messages.iter().map(|message| {
                (
                    message.id,
                    self.rendered_texts_by_id[&message.id].text.as_ref(),
                )
            }),
        );
        if reset
            || !self
                .find
                .active
                .is_some_and(|id| self.find.matches.contains(&id))
        {
            self.find.active = self.find.matches.first().copied();
        }
        self.invalidate_find_rows();
        if reset && let Some(id) = self.find.active {
            self.scroll_to(&id);
        }
        cx.notify();
    }

    fn invalidate_find_rows(&self) {
        // List caches rendered rows; changing the active result must redraw them.
        let top = self.list_state.logical_scroll_top();
        self.list_state
            .splice(0..self.messages.len(), self.messages.len());
        self.list_state.scroll_to(top);
    }

    fn move_find(&mut self, previous: bool, cx: &mut Context<Self>) {
        self.find.active = next_match(&self.find.matches, self.find.active, previous);
        self.invalidate_find_rows();
        if let Some(id) = self.find.active {
            self.scroll_to(&id);
        }
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.open = false;
        self.invalidate_find_rows();
        if let Some(focus) = self.find.return_focus.take() {
            window.focus(&focus, cx);
        } else {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    pub(super) fn escape_find(
        &mut self,
        _: &ui::input::Escape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.find.open
            && self
                .find
                .input
                .focus_handle(cx)
                .contains_focused(window, cx)
        {
            self.close_find(window, cx);
        } else {
            cx.propagate();
        }
    }

    pub(super) fn subscribe_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.subscriptions.push(cx.subscribe_in(
            &self.find.input,
            window,
            |this, _, event, _, cx| match event {
                InputEvent::Change => this.refresh_find(true, cx),
                InputEvent::PressEnter { shift, .. } => this.move_find(*shift, cx),
                _ => {}
            },
        ));
    }

    pub(super) fn render_find(&self, cx: &Context<Self>) -> impl IntoElement {
        let count = self.find.matches.len();
        let position = self
            .find
            .active
            .and_then(|id| self.find.matches.iter().position(|item| *item == id));
        let label = if self.find.input.read(cx).value().is_empty() {
            String::new()
        } else if count == 0 {
            "No matches".into()
        } else {
            format!(
                "{} of {} messages",
                position.map_or(0, |index| index + 1),
                count
            )
        };
        h_flex()
            .flex_shrink_0()
            .w_full()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(Input::new(&self.find.input).small().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().text_muted)
                    .child(label),
            )
            .child(
                Button::new("find-previous")
                    .icon(IconName::CaretUp)
                    .small()
                    .ghost()
                    .tooltip("Previous match (Shift+Enter)")
                    .disabled(count == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.move_find(true, cx))),
            )
            .child(
                Button::new("find-next")
                    .icon(IconName::CaretDown)
                    .small()
                    .ghost()
                    .tooltip("Next match (Enter)")
                    .disabled(count == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.move_find(false, cx))),
            )
            .child(
                Button::new("close-find")
                    .icon(IconName::Close)
                    .small()
                    .ghost()
                    .tooltip("Close search (Esc)")
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: u8) -> EventId {
        EventId::from_byte_array([n; 32])
    }
    #[test]
    fn finds_literal_case_insensitive_text_without_empty_query_matches() {
        let messages = [
            (id(1), "Hello Alice"),
            (id(2), "HELLO again"),
            (id(3), "a.*b"),
            (id(4), "CAFÉ"),
        ];
        assert_eq!(
            matching_messages("hello", messages.iter().copied()),
            vec![id(1), id(2)]
        );
        assert_eq!(
            matching_messages(".*", messages.iter().copied()),
            vec![id(3)]
        );
        assert_eq!(
            matching_messages("café", messages.iter().copied()),
            vec![id(4)]
        );
        assert!(matching_messages("", messages.iter().copied()).is_empty());
        assert!(matching_messages("absent", messages.iter().copied()).is_empty());
    }
    #[test]
    fn wraps_in_both_directions_and_handles_removed_selection() {
        let ids = [id(1), id(2), id(3)];
        assert_eq!(next_match(&ids, Some(id(3)), false), Some(id(1)));
        assert_eq!(next_match(&ids, Some(id(1)), true), Some(id(3)));
        assert_eq!(next_match(&ids, Some(id(9)), false), Some(id(1)));
        assert_eq!(next_match(&[], None, true), None);
        assert_eq!(next_match(&[id(1)], Some(id(1)), false), Some(id(1)));
    }
}
