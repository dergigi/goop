use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc};

use gpui::{
    AnyElement, App, ClipboardItem, DispatchPhase, ElementId, FocusHandle, MouseButton,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, StyledText, Window, canvas, div,
    prelude::*, px,
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub(super) struct TextSelection {
    focus: Option<FocusHandle>,
    anchor: Range<usize>,
    pub range: Range<usize>,
    dragging: bool,
    clicks: usize,
    down: Point<Pixels>,
    moved: bool,
    link_hovered: bool,
}

impl TextSelection {
    pub fn visible_range(&self, window: &Window) -> Range<usize> {
        if self
            .focus
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window))
        {
            self.range.clone()
        } else {
            0..0
        }
    }

    fn begin(&mut self, text: &str, index: usize, clicks: usize, shift: bool) {
        self.clicks = clicks;
        self.moved = shift;
        self.dragging = true;
        if shift {
            self.extend(text, index);
        } else {
            self.anchor = unit(text, index, clicks);
            self.range = self.anchor.clone();
        }
    }

    fn extend(&mut self, text: &str, index: usize) {
        let target = unit(text, index, self.clicks);
        self.range = target.start.min(self.anchor.start)..target.end.max(self.anchor.end);
    }
}

fn unit(text: &str, index: usize, clicks: usize) -> Range<usize> {
    let index = index.min(text.len());
    if clicks >= 3 {
        let start = text[..index].rfind('\n').map_or(0, |i| i + 1);
        let end = text[index..]
            .find('\n')
            .map_or(text.len(), |i| index + i + 1);
        start..end
    } else if clicks == 2 {
        text.split_word_bound_indices()
            .find(|(start, word)| *start <= index && index < start + word.len())
            .map_or(index..index, |(start, word)| start..start + word.len())
    } else {
        // Keep selection boundaries outside a multi-codepoint grapheme (emoji, accents).
        let index = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i >= index)
            .unwrap_or(text.len());
        index..index
    }
}

pub(super) fn selectable(
    id: ElementId,
    text: StyledText,
    content: SharedString,
    links: Vec<(Range<usize>, String)>,
    state: Rc<RefCell<TextSelection>>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let owner = window.current_view();
    let focus = state
        .borrow_mut()
        .focus
        .get_or_insert_with(|| cx.focus_handle())
        .clone();
    let layout = text.layout().clone();
    let links: Arc<[(Range<usize>, String)]> = links.into();
    let link_hovered = state.borrow().link_hovered;
    div()
        .id(id)
        .track_focus(&focus)
        .key_context("MessageText")
        .cursor_text()
        .when(link_hovered, |this| this.cursor_pointer())
        .on_mouse_move({
            let state = state.clone();
            let layout = layout.clone();
            let links = links.clone();
            move |event, _window, cx| {
                let hovered = layout
                    .index_for_position(event.position)
                    .ok()
                    .is_some_and(|index| links.iter().any(|(range, _)| range.contains(&index)));
                let mut state = state.borrow_mut();
                if hovered != state.link_hovered {
                    state.link_hovered = hovered;
                    cx.notify(owner);
                }
            }
        })
        .child(text)
        .on_mouse_down(MouseButton::Left, {
            let state = state.clone();
            let layout = layout.clone();
            let content = content.clone();
            move |event, window, cx| {
                let index = layout
                    .index_for_position(event.position)
                    .unwrap_or_else(|i| i);
                let mut state = state.borrow_mut();
                state.down = event.position;
                state.begin(&content, index, event.click_count, event.modifiers.shift);
                focus.focus(window, cx);
                cx.notify(owner);
                cx.stop_propagation();
            }
        })
        .on_mouse_down_out({
            let state = state.clone();
            move |event, _window, cx| {
                if event.button == MouseButton::Left {
                    let mut state = state.borrow_mut();
                    if !state.range.is_empty() || state.dragging {
                        state.range = 0..0;
                        state.dragging = false;
                        cx.notify(owner);
                    }
                }
            }
        })
        .on_key_down({
            let state = state.clone();
            let content = content.clone();
            move |event, _window, cx| {
                let modifiers = event.keystroke.modifiers;
                let command = if cfg!(target_os = "macos") {
                    modifiers.platform && !modifiers.control
                } else {
                    modifiers.control && !modifiers.platform
                };
                if !command || modifiers.alt || modifiers.shift {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "c" => {
                        let range = state.borrow().range.clone();
                        if !range.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                content[range].to_string(),
                            ));
                            cx.stop_propagation();
                        }
                    }
                    "a" => {
                        let mut state = state.borrow_mut();
                        state.range = 0..content.len();
                        state.anchor = 0..0;
                        state.clicks = 1;
                        cx.notify(owner);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }
        })
        .child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    window.on_mouse_event({
                        let state = state.clone();
                        let layout = layout.clone();
                        let content = content.clone();
                        move |event: &MouseMoveEvent, phase, _window, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            let mut state = state.borrow_mut();
                            if !state.dragging {
                                return;
                            }
                            if !event.dragging() {
                                state.dragging = false;
                                return;
                            }
                            if (event.position.x - state.down.x).abs() > px(3.)
                                || (event.position.y - state.down.y).abs() > px(3.)
                            {
                                state.moved = true;
                            }
                            if state.moved {
                                let index = layout
                                    .index_for_position(event.position)
                                    .unwrap_or_else(|i| i);
                                state.extend(&content, index);
                                cx.notify(owner);
                                cx.stop_propagation();
                            }
                        }
                    });
                    window.on_mouse_event({
                        let state = state.clone();
                        let layout = layout.clone();
                        let links = links.clone();
                        move |event: &MouseUpEvent, phase, _window, cx| {
                            if phase != DispatchPhase::Capture || event.button != MouseButton::Left
                            {
                                return;
                            }
                            let mut state = state.borrow_mut();
                            if !state.dragging {
                                return;
                            }
                            state.dragging = false;
                            if !state.moved && state.clicks == 1 && state.range.is_empty() {
                                if let Ok(index) = layout.index_for_position(event.position) {
                                    if let Some((_, url)) = links.iter().find(|(range, _)| {
                                        range.contains(&index)
                                            && range.contains(&state.anchor.start)
                                    }) {
                                        if super::text::is_web_url(url) {
                                            cx.open_url(url);
                                        }
                                    }
                                }
                            }
                            cx.notify(owner);
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .size_0(),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use gpui::{
        Context, Modifiers, MouseDownEvent, Render, TestAppContext, TextLayout, VisualTestContext,
        point,
    };
    #[test]
    fn words_and_paragraphs_use_displayed_unicode_text() {
        let text = "Hello, café 👨‍👩‍👧‍👦!\nNext line";
        assert_eq!(&text[unit(text, 8, 2)], "café");
        assert_eq!(&text[unit(text, 14, 2)], "👨‍👩‍👧‍👦");
        assert_eq!(&text[unit(text, 8, 3)], "Hello, café 👨‍👩‍👧‍👦!\n");
        assert_eq!(unit(text, text.len(), 1), text.len()..text.len());
    }
    #[test]
    fn selection_extends_in_both_directions_and_preserves_word_anchor() {
        let mut state = TextSelection::default();
        let text = "one two three";
        state.begin(text, 5, 2, false);
        assert_eq!(state.range, 4..7);
        state.extend(text, 11);
        assert_eq!(state.range, 4..13);
        state.extend(text, 1);
        assert_eq!(state.range, 0..7);
        state.begin(text, 5, 1, false);
        state.extend(text, 2);
        assert_eq!(state.range, 2..5);
        state.begin(text, 9, 1, true);
        assert_eq!(state.range, 5..9);
    }
    #[cfg(feature = "test-support")]
    #[gpui::test]
    fn mouse_selection_and_copy(cx: &mut TestAppContext) {
        struct Harness {
            selection: Rc<RefCell<TextSelection>>,
            layout: TextLayout,
        }
        impl Render for Harness {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let text = StyledText::new("one two\nthree");
                self.layout = text.layout().clone();
                div().w(px(40.)).text_size(px(16.)).child(selectable(
                    "message".into(),
                    text,
                    "one two\nthree".into(),
                    vec![(0..13, "https://goop.dergigi.com".into())],
                    self.selection.clone(),
                    window,
                    cx,
                ))
            }
        }
        let selection = Rc::new(RefCell::new(TextSelection::default()));
        let (view, cx) = cx.add_window_view(|_, _| Harness {
            selection: selection.clone(),
            layout: TextLayout::default(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let point_at = |index, cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                view.read(cx).layout.position_for_index(index).unwrap() + point(px(1.), px(5.))
            })
        };
        let word = point_at(5, cx);
        assert!(word.y > point_at(0, cx).y, "fixture must wrap its first line");
        cx.simulate_event(MouseDownEvent {
            position: word,
            button: MouseButton::Left,
            click_count: 2,
            ..Default::default()
        });
        cx.simulate_mouse_up(word, MouseButton::Left, Modifiers::default());
        assert_eq!(selection.borrow().range, 4..7);
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
        cx.update(|_, cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), "two"));
        let start = point_at(1, cx);
        let end = point_at(11, cx);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        assert_eq!(selection.borrow().range, 1..11);
        // Drag outside the text and release there, then moving back must not extend it.
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        let outside = point(px(350.), px(150.));
        cx.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        assert_eq!(selection.borrow().range, 1..13);
        cx.simulate_mouse_move(word, None, Modifiers::default());
        assert_eq!(selection.borrow().range, 1..13);
        assert!(
            cx.opened_url().is_none(),
            "selecting link text must not open it"
        );
        cx.simulate_click(word, Modifiers::default());
        assert_eq!(cx.opened_url().as_deref(), Some("https://goop.dergigi.com"));
    }
}
