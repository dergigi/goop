//! Shared, searchable Unicode emoji picker for composition and reactions.
use std::{collections::HashSet, rc::Rc, sync::LazyLock};

use emojis::{Emoji, Group, SkinTone};
use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, AnyElement, App, AppContext, Context, ElementId, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, RenderOnce,
    ScrollStrategy, SharedString, Styled, Subscription, UniformListScrollHandle, Window, div, px,
    uniform_list,
};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::input::{Input, InputEvent, InputState};
use ui::popover::Popover;
use ui::{Selectable, Sizable, h_flex, v_flex};

const COLUMNS: usize = 8;
const GROUPS: [(Group, &str, &str); 9] = [
    (Group::SmileysAndEmotion, "😀", "Smileys & emotion"),
    (Group::PeopleAndBody, "👋", "People & body"),
    (Group::AnimalsAndNature, "🐻", "Animals & nature"),
    (Group::FoodAndDrink, "🍎", "Food & drink"),
    (Group::TravelAndPlaces, "🚗", "Travel & places"),
    (Group::Activities, "⚽", "Activities"),
    (Group::Objects, "💡", "Objects"),
    (Group::Symbols, "🔣", "Symbols"),
    (Group::Flags, "🏳️", "Flags"),
];
const TONES: [(Option<SkinTone>, &str); 7] = [
    (Some(SkinTone::Default), "✋ Default"),
    (Some(SkinTone::Light), "✋🏻 Light"),
    (Some(SkinTone::MediumLight), "✋🏼 Medium-light"),
    (Some(SkinTone::Medium), "✋🏽 Medium"),
    (Some(SkinTone::MediumDark), "✋🏾 Medium-dark"),
    (Some(SkinTone::Dark), "✋🏿 Dark"),
    (None, "All skin tones"),
];

struct Entry {
    emoji: &'static Emoji,
    search: String,
}
static CATALOG: LazyLock<Vec<Entry>> = LazyLock::new(|| {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for base in emojis::iter() {
        let variants: Vec<_> = base
            .skin_tones()
            .map(|v| v.collect())
            .unwrap_or_else(|| vec![base]);
        for emoji in variants {
            if seen.insert(emoji.as_str()) {
                let aliases = base
                    .shortcodes()
                    .chain(emoji.shortcodes())
                    .collect::<Vec<_>>()
                    .join(" ");
                entries.push(Entry {
                    emoji,
                    search: format!("{} {} {}", emoji.as_str(), emoji.name(), aliases)
                        .to_lowercase()
                        .replace(['_', '-'], " "),
                });
            }
        }
    }
    entries
});

fn results(query: &str, group: Option<Group>, tone: Option<SkinTone>) -> Vec<&'static Emoji> {
    let query = query
        .trim()
        .trim_matches(':')
        .to_lowercase()
        .replace(['_', '-'], " ");
    let words: Vec<_> = query.split_whitespace().collect();
    if !words.is_empty() {
        // Search all groups and variants, including mixed skin tones.
        return CATALOG
            .iter()
            .filter(|entry| words.iter().all(|word| entry.search.contains(word)))
            .map(|entry| entry.emoji)
            .collect();
    }
    if tone.is_none() {
        return CATALOG
            .iter()
            .filter(|entry| group.is_none_or(|group| entry.emoji.group() == group))
            .map(|entry| entry.emoji)
            .collect();
    }
    emojis::iter()
        .filter(|emoji| group.is_none_or(|group| emoji.group() == group))
        .map(|emoji| emoji.with_skin_tone(tone.unwrap()).unwrap_or(emoji))
        .collect()
}

fn move_selection(selected: usize, len: usize, delta: isize) -> usize {
    selected
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

#[derive(Clone, Copy)]
struct Choice(Option<&'static str>);
impl EventEmitter<Choice> for EmojiPicker {}

struct EmojiPicker {
    input: Entity<InputState>,
    grid_focus: FocusHandle,
    group: Option<Group>,
    tone: Option<SkinTone>,
    visible: Vec<&'static Emoji>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _input_subscription: Subscription,
}
impl EmojiPicker {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search emoji"));
        let subscription = cx.subscribe_in(&input, window, |this, _, event, _, cx| match event {
            InputEvent::Change => this.filter(cx),
            InputEvent::PressEnter { .. } => this.choose(cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        Self {
            input,
            grid_focus: cx.focus_handle(),
            group: None,
            tone: Some(SkinTone::Default),
            visible: results("", None, Some(SkinTone::Default)),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _input_subscription: subscription,
        }
    }
    fn filter(&mut self, cx: &mut Context<Self>) {
        self.visible = results(self.input.read(cx).value().as_ref(), self.group, self.tone);
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }
    fn choose(&self, cx: &mut Context<Self>) {
        if let Some(emoji) = self.visible.get(self.selected) {
            cx.emit(Choice(Some(emoji.as_str())));
        }
    }
    fn move_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.selected = move_selection(self.selected, self.visible.len(), delta);
        self.scroll
            .scroll_to_item(self.selected / COLUMNS, ScrollStrategy::Center);
        cx.notify();
    }
    fn rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        range
            .map(|row| {
                h_flex()
                    .h_9()
                    .w_full()
                    .children(
                        (row * COLUMNS..((row + 1) * COLUMNS).min(self.visible.len())).map(
                            |index| {
                                let emoji = self.visible[index];
                                Button::new(SharedString::from(emoji.as_str()))
                                    .label(emoji.as_str())
                                    .tooltip(emoji.name().to_owned())
                                    .ghost()
                                    .selected(index == self.selected)
                                    .w_9()
                                    .h_9()
                                    .text_size(px(23.))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(Choice(Some(emoji.as_str())))
                                    }))
                            },
                        ),
                    )
                    .into_any_element()
            })
            .collect()
    }
}
impl Render for EmojiPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let grid_height = (window.viewport_size().height - px(230.))
            .min(px(252.))
            .max(px(72.));
        let current_group = self.group;
        let current_tone = self.tone;
        v_flex()
            .debug_selector(|| "emoji-picker".into())
            .w(px(288.))
            .gap_2()
            .visible()
            .track_focus(&self.grid_focus)
            .capture_action(cx.listener(|this, _: &ui::input::Enter, _, cx| {
                cx.stop_propagation();
                this.choose(cx);
            }))
            .capture_action(cx.listener(|_, _: &ui::input::Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(Choice(None));
            }))
            .capture_action(cx.listener(|this, _: &ui::input::MoveDown, window, cx| {
                cx.stop_propagation();
                this.grid_focus.focus(window, cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if !this.grid_focus.is_focused(window) {
                    return;
                }
                let delta = match event.keystroke.key.as_str() {
                    "left" => Some(-1),
                    "right" => Some(1),
                    "up" => Some(-(COLUMNS as isize)),
                    "down" => Some(COLUMNS as isize),
                    _ => None,
                };
                if let Some(delta) = delta {
                    this.move_by(delta, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "enter" {
                    this.choose(cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape" {
                    cx.emit(Choice(None));
                    cx.stop_propagation();
                }
            }))
            .child(Input::new(&self.input).small())
            .child(
                h_flex()
                    .w_full()
                    .child(
                        Button::new("all-emoji")
                            .label("All")
                            .tooltip("All emoji")
                            .xsmall()
                            .ghost()
                            .selected(current_group.is_none())
                            .w_7()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.group = None;
                                this.input
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                                this.filter(cx);
                                this.input.update(cx, |input, cx| input.focus(window, cx));
                            })),
                    )
                    .children(GROUPS.into_iter().enumerate().map(
                        |(index, (group, icon, name))| {
                            Button::new(("emoji-group", index))
                                .label(icon)
                                .tooltip(name)
                                .xsmall()
                                .ghost()
                                .w_7()
                                .selected(current_group == Some(group))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.group = Some(group);
                                    this.input
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                    this.filter(cx);
                                    this.input.update(cx, |input, cx| input.focus(window, cx));
                                }))
                        },
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .text_xs()
                    .text_color(cx.theme().text_muted)
                    .child(div().flex_1().child("Skin tone"))
                    .children(TONES.into_iter().enumerate().map(|(index, (tone, label))| {
                        Button::new(("emoji-tone", index))
                            .label(if tone.is_none() {
                                "All"
                            } else {
                                label.split_whitespace().next().unwrap()
                            })
                            .tooltip(label)
                            .xsmall()
                            .ghost()
                            .w_7()
                            .selected(current_tone == tone)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.tone = tone;
                                this.filter(cx);
                                this.input.update(cx, |input, cx| input.focus(window, cx));
                            }))
                    })),
            )
            .child(
                div()
                    .debug_selector(|| "emoji-picker-grid".into())
                    .h(grid_height)
                    .w_full()
                    .when(self.visible.is_empty(), |view| {
                        view.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(cx.theme().text_muted)
                                .child("No matching emoji"),
                        )
                    })
                    .when(!self.visible.is_empty(), |view| {
                        view.child(
                            uniform_list(
                                "emoji-grid",
                                self.visible.len().div_ceil(COLUMNS),
                                cx.processor(|this, range, _, cx| this.rows(range, cx)),
                            )
                            .track_scroll(&self.scroll)
                            .size_full(),
                        )
                    }),
            )
            .child(
                div()
                    .h_5()
                    .text_xs()
                    .text_color(cx.theme().text_muted)
                    .truncate()
                    .child(
                        self.visible
                            .get(self.selected)
                            .map(|emoji| emoji.name().to_owned())
                            .unwrap_or_default(),
                    ),
            )
    }
}

type Callback = Rc<dyn Fn(Option<&'static str>, &mut Window, &mut App)>;
#[derive(IntoElement)]
pub struct EmojiPopover {
    id: ElementId,
    trigger: Button,
    callback: Callback,
}
impl EmojiPopover {
    pub fn new(
        id: impl Into<ElementId>,
        trigger: Button,
        callback: impl Fn(Option<&'static str>, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            trigger,
            callback: Rc::new(callback),
        }
    }
}
impl RenderOnce for EmojiPopover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| None::<Entity<EmojiPicker>>);
        let close_state = state.clone();
        let callback = self.callback;
        Popover::new(self.id)
            .anchor(Anchor::BottomLeft)
            .trigger(self.trigger)
            .on_open_change(move |open, _, cx| {
                if !open {
                    close_state.update(cx, |picker, _| *picker = None);
                }
            })
            .content(move |_, window, cx| {
                if let Some(picker) = state.read(cx).clone() {
                    return picker;
                }
                let picker = cx.new(|cx| EmojiPicker::new(window, cx));
                let popover = cx.weak_entity();
                let callback = callback.clone();
                window
                    .subscribe(&picker, cx, move |_, choice: &Choice, window, cx| {
                        popover
                            .update(cx, |popover, cx| popover.dismiss(window, cx))
                            .ok();
                        callback(choice.0, window, cx);
                    })
                    .detach();
                state.update(cx, |state, _| *state = Some(picker.clone()));
                picker
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_contains_every_base_and_skin_variant_once() {
        let actual: HashSet<_> = results("", None, None).iter().map(|e| e.as_str()).collect();
        assert_eq!(actual.len(), CATALOG.len());
        assert!(actual.len() > 3900);
        for base in emojis::iter() {
            assert!(actual.contains(base.as_str()));
            if let Some(variants) = base.skin_tones() {
                for variant in variants {
                    assert!(actual.contains(variant.as_str()));
                }
            }
        }
        for emoji in [
            "🫩",
            "👩🏿‍❤️‍👨🏼",
            "🇵🇹",
            "👨‍👩‍👧‍👦",
            "👍🏽",
            "1️⃣",
            "🏴\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}",
        ] {
            assert!(actual.contains(emoji), "missing {emoji}");
        }
    }
    #[test]
    fn search_matches_names_shortcodes_and_variants_across_categories() {
        for (query, emoji) in [
            (":rocket:", "🚀"),
            ("thumbs_up", "👍"),
            ("portugal", "🇵🇹"),
            ("👍🏽", "👍🏽"),
            ("medium dark", "👍🏾"),
        ] {
            assert!(
                results(query, Some(Group::FoodAndDrink), Some(SkinTone::Default))
                    .iter()
                    .any(|e| e.as_str() == emoji),
                "{query}"
            );
        }
        assert!(results("notanemojixyz", None, None).is_empty());
    }
    #[test]
    fn category_and_tone_selection_keep_unmodified_emoji() {
        assert!(
            results("", Some(Group::Flags), Some(SkinTone::Dark))
                .iter()
                .all(|e| e.group() == Group::Flags)
        );
        assert!(
            results("", Some(Group::PeopleAndBody), Some(SkinTone::Dark))
                .iter()
                .any(|e| e.as_str() == "👍🏿")
        );
        assert!(
            results("", None, Some(SkinTone::Dark))
                .iter()
                .any(|e| e.as_str() == "🚀")
        );
    }
    #[test]
    fn selection_clamps_in_empty_and_partial_rows() {
        assert_eq!(move_selection(0, 0, 8), 0);
        assert_eq!(move_selection(0, 12, -1), 0);
        assert_eq!(move_selection(7, 12, 8), 11);
        assert_eq!(move_selection(11, 12, 1), 11);
    }
}

#[cfg(all(test, feature = "test-support"))]
mod interaction_tests {
    use super::*;
    use gpui::TestAppContext;
    use std::cell::RefCell;

    struct ComposerHarness {
        input: Entity<InputState>,
        choices: Rc<RefCell<Vec<Option<&'static str>>>>,
    }
    impl Render for ComposerHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let input = self.input.clone();
            let choices = self.choices.clone();
            v_flex().p_4().gap_2().child(Input::new(&self.input)).child(
                div()
                    .w_9()
                    .h_9()
                    .group("test-emoji-row")
                    .debug_selector(|| "emoji-test-trigger".into())
                    .child(
                        div()
                            .invisible()
                            .group_hover("test-emoji-row", |style| style.visible())
                            .child(EmojiPopover::new(
                                "test-emoji-popover",
                                Button::new("test-emoji-button").label("😀").w_9().h_9(),
                                move |choice, window, cx| {
                                    choices.borrow_mut().push(choice);
                                    input.update(cx, |input, cx| {
                                        if let Some(emoji) = choice {
                                            input.replace(emoji, window, cx);
                                        }
                                        input.focus(window, cx);
                                    });
                                },
                            )),
                    ),
            )
        }
    }

    #[gpui::test]
    fn popover_survives_hover_exit_and_restores_composer_on_pick_and_cancel(
        cx: &mut TestAppContext,
    ) {
        use gpui::{Modifiers, point};
        let choices = Rc::new(RefCell::new(Vec::new()));
        let (root, cx) = cx.add_window_view(|window, cx| {
            theme::init(cx);
            ui::init(cx);
            let view = cx.new(|cx| ComposerHarness {
                input: cx.new(|cx| InputState::new(window, cx)),
                choices: choices.clone(),
            });
            ui::Root::new(view.into(), window, cx)
        });
        let input = cx.update(|window, cx| {
            let view = root
                .read(cx)
                .view()
                .clone()
                .downcast::<ComposerHarness>()
                .unwrap();
            let input = view.read(cx).input.clone();
            input.update(cx, |input, cx| input.focus(window, cx));
            window.draw(cx).clear(cx);
            input
        });
        cx.simulate_input("AB");
        cx.simulate_keystrokes("left");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let trigger = cx.debug_bounds("emoji-test-trigger").unwrap().center();
        cx.simulate_mouse_move(trigger, None, Modifiers::default());
        cx.simulate_click(trigger, Modifiers::default());
        cx.simulate_input("rocket");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let grid = cx
            .debug_bounds("emoji-picker-grid")
            .expect("picker is open");
        let first = grid.origin + point(px(18.), px(18.));
        cx.simulate_mouse_move(first, None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_click(first, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(*choices.borrow(), vec![Some("🚀")]);
        cx.simulate_input("!");
        cx.update(|_, cx| assert_eq!(input.read(cx).value().as_ref(), "A🚀!B"));
        for _ in 0..2 {
            cx.simulate_mouse_move(trigger, None, Modifiers::default());
            cx.simulate_click(trigger, Modifiers::default());
            cx.simulate_input("rocket");
            cx.simulate_keystrokes("escape");
        }
        assert_eq!(*choices.borrow(), vec![Some("🚀"), None, None]);
        cx.simulate_input("?");
        cx.update(|_, cx| assert_eq!(input.read(cx).value().as_ref(), "A🚀!?B"));
    }

    #[gpui::test]
    fn keyboard_search_grid_selection_and_escape(cx: &mut TestAppContext) {
        let choices = Rc::new(RefCell::new(Vec::new()));
        let (root, cx) = cx.add_window_view(|window, cx| {
            theme::init(cx);
            ui::init(cx);
            let picker = cx.new(|cx| EmojiPicker::new(window, cx));
            ui::Root::new(picker.into(), window, cx)
        });
        let picker = cx.update(|_, cx| {
            root.read(cx)
                .view()
                .clone()
                .downcast::<EmojiPicker>()
                .unwrap()
        });
        cx.update(|window, cx| {
            let choices = choices.clone();
            window
                .subscribe(&picker, cx, move |_, choice: &Choice, _, _| {
                    choices.borrow_mut().push(choice.0)
                })
                .detach();
            window.draw(cx).clear(cx);
        });
        cx.simulate_input("rocket");
        cx.simulate_keystrokes("enter");
        assert_eq!(*choices.borrow(), vec![Some("🚀")]);
        cx.simulate_keystrokes("escape");
        assert_eq!(*choices.borrow(), vec![Some("🚀"), None]);
        cx.update(|window, cx| {
            picker.update(cx, |picker, cx| {
                picker
                    .input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                picker.filter(cx);
            })
        });
        cx.simulate_keystrokes("down right right down enter");
        cx.update(|window, cx| {
            assert!(picker.read(cx).grid_focus.is_focused(window));
            assert_eq!(picker.read(cx).selected, 10);
            assert_eq!(
                choices.borrow().last().copied(),
                Some(Some(picker.read(cx).visible[10].as_str()))
            );
        });
    }
}
