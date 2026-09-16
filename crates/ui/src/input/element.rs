use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Corners, Edges, Element, ElementId, ElementInputHandler, Entity,
    GlobalElementId, Half, Hsla, IntoElement, LayoutId, MouseButton, MouseMoveEvent, MouseUpEvent,
    Path, Pixels, Point, Position, SharedString, Size, Style, TextAlign, TextRun, TextStyle,
    UnderlineStyle, Window, fill, point, px, relative, size,
};
use ropey::Rope;
use smallvec::SmallVec;
use theme::ActiveTheme;

use super::mode::InputMode;
use super::{InputState, LastLayout, WhitespaceIndicators};
use crate::Root;
use crate::input::RopeExt as _;
use crate::input::blink_cursor::CURSOR_WIDTH;
use crate::input::display_map::LineLayout;
use crate::scroll::Scrollbar;

const BOTTOM_MARGIN_ROWS: usize = 3;
pub(super) const RIGHT_MARGIN: Pixels = px(10.);

#[derive(Clone, Copy, Debug, PartialEq)]
struct EditorScrollbarLayout {
    bounds: Bounds<Pixels>,
    scroll_size: Size<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct EditorScrollbarSnapshot {
    layout: EditorScrollbarLayout,
    cursor_scroll_offset: Point<Pixels>,
    soft_wrap: bool,
}

impl EditorScrollbarSnapshot {
    fn new(
        input_bounds: Bounds<Pixels>,
        last_layout: &LastLayout,
        scroll_size: Size<Pixels>,
        cursor_scroll_offset: Point<Pixels>,
        state: &InputState,
    ) -> Self {
        Self {
            layout: EditorScrollbarLayout::new(
                input_bounds,
                last_layout.line_number_width,
                scroll_size,
                state.editor_scrollbar_paddings.get(),
            ),
            cursor_scroll_offset,
            soft_wrap: state.soft_wrap,
        }
    }
}

impl EditorScrollbarLayout {
    fn new(
        input_bounds: Bounds<Pixels>,
        line_number_width: Pixels,
        scroll_size: Size<Pixels>,
        paddings: Edges<Pixels>,
    ) -> Self {
        let left = if line_number_width == px(0.) {
            px(0.)
        } else {
            paddings.left + line_number_width
        };

        Self {
            bounds: Bounds::new(
                point(
                    input_bounds.origin.x + left,
                    input_bounds.origin.y - paddings.top,
                ),
                size(
                    input_bounds.size.width - left + paddings.right,
                    input_bounds.size.height + paddings.top + paddings.bottom,
                ),
            ),
            scroll_size: size(
                scroll_size.width - left + paddings.right + RIGHT_MARGIN,
                scroll_size.height,
            ),
        }
    }
}

pub(super) struct EditorScrollbar {
    state: Entity<InputState>,
}

impl EditorScrollbar {
    pub(super) fn new(state: Entity<InputState>) -> Self {
        Self { state }
    }
}

impl IntoElement for EditorScrollbar {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorScrollbar {
    type PrepaintState = Option<AnyElement>;
    type RequestLayoutState = ();

    fn id(&self) -> Option<ElementId> {
        Some("editor-scrollbar".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            position: Position::Absolute,
            size: Size {
                width: relative(1.).into(),
                height: relative(1.).into(),
            },
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let state = self.state.read(cx);
        let snapshot = state.editor_scrollbar_snapshot.get()?;
        let scroll_handle = state.scroll_handle.clone();

        if scroll_handle.offset() != snapshot.cursor_scroll_offset {
            scroll_handle.set_offset(snapshot.cursor_scroll_offset);
        }

        let mut scrollbar = if !snapshot.soft_wrap {
            Scrollbar::new(&scroll_handle)
        } else {
            Scrollbar::vertical(&scroll_handle)
        }
        .scroll_size(snapshot.layout.scroll_size)
        .into_any_element();

        scrollbar.prepaint_as_root(
            snapshot.layout.bounds.origin,
            snapshot.layout.bounds.size.into(),
            window,
            cx,
        );
        Some(scrollbar)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(scrollbar) = prepaint.as_mut() {
            scrollbar.paint(window, cx);
        }
    }
}

fn clamp_auto_grow_vertical_scroll_offset(
    mode: &InputMode,
    scroll_top: Pixels,
    scroll_height: Pixels,
    input_height: Pixels,
) -> Pixels {
    if mode.is_auto_grow() {
        scroll_top.clamp((input_height - scroll_height).min(px(0.)), px(0.))
    } else {
        scroll_top
    }
}

use super::MASK_CHAR;

/// Convert a byte offset in the original text to a byte offset in the masked display string.
///
/// The masked string consists of `MASK_CHAR` repeated once per character in the original text.
/// Since `MASK_CHAR` may be multi-byte in UTF-8, the byte offset in the masked string is
/// `char_index * MASK_CHAR.len_utf8()`.
fn masked_display_offset(text: &Rope, original_offset: usize) -> usize {
    text.offset_to_char_index(original_offset) * MASK_CHAR.len_utf8()
}

pub(super) struct TextElement {
    pub(crate) state: Entity<InputState>,
    placeholder: SharedString,
}

impl TextElement {
    pub(super) fn new(state: Entity<InputState>) -> Self {
        Self {
            state,
            placeholder: SharedString::default(),
        }
    }

    /// Set the placeholder text of the input field.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    fn paint_mouse_listeners(&mut self, window: &mut Window, _: &mut App) {
        window.on_mouse_event({
            let state = self.state.clone();

            move |event: &MouseMoveEvent, _, window, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    state.update(cx, |state, cx| {
                        state.on_drag_move(event, window, cx);
                    });
                }
            }
        });

        window.on_mouse_event({
            let state = self.state.clone();
            move |_: &MouseUpEvent, phase, _, cx| {
                if !phase.bubble() {
                    return;
                }

                // Stop auto-scroll when mouse up, and also stop selecting.
                state.update(cx, |state, _| {
                    state.selecting = false;
                });
            }
        });
    }

    /// Returns the:
    ///
    /// - cursor bounds
    /// - scroll offset
    /// - current row index (No only the visible lines, but all lines)
    ///
    /// This method also will update for track scroll to cursor.
    fn layout_cursor(
        &self,
        last_layout: &LastLayout,
        bounds: &mut Bounds<Pixels>,
        scroll_size: Size<Pixels>,
        _: &mut Window,
        cx: &mut App,
    ) -> (Option<Bounds<Pixels>>, Point<Pixels>) {
        let state = self.state.read(cx);

        let line_height = last_layout.line_height;
        let visible_range = &last_layout.visible_range;
        let lines = &last_layout.lines;
        let line_number_width = last_layout.line_number_width;

        let mut selected_range = state.selected_range;

        if let Some(ime_marked_range) = &state.ime_marked_range {
            selected_range = (ime_marked_range.end..ime_marked_range.end).into();
        }
        let is_selected_all = selected_range.len() == state.text.len();

        let mut cursor = state.cursor();
        if state.masked {
            selected_range.start = masked_display_offset(&state.text, selected_range.start);
            selected_range.end = masked_display_offset(&state.text, selected_range.end);
            cursor = masked_display_offset(&state.text, cursor);
        }

        let mut scroll_offset = state.scroll_handle.offset();
        let mut cursor_bounds = None;

        // If the input has a fixed height (Otherwise is auto-grow), we need to add a bottom margin to the input.
        let top_bottom_margin =
            if state.mode.is_auto_grow() || visible_range.len() < BOTTOM_MARGIN_ROWS * 8 {
                line_height
            } else {
                BOTTOM_MARGIN_ROWS * line_height
            };

        // The cursor corresponds to the current cursor position in the text no only the line.
        let mut cursor_pos = None;
        let mut cursor_start = None;
        let mut cursor_end = None;

        let mut prev_lines_offset = 0;
        let mut offset_y = px(0.);
        let buffer_lines = state.display_map.lines();
        let visible_buffer_lines = &last_layout.visible_buffer_lines;
        let mut vi = 0; // index into visible_buffer_lines / lines
        for (ix, wrap_line) in buffer_lines.iter().enumerate() {
            let line_origin = point(px(0.), offset_y);

            // break loop if all cursor positions are found
            if cursor_pos.is_some() && cursor_start.is_some() && cursor_end.is_some() {
                break;
            }

            // Check if this buffer line has a LineLayout in the compact lines vec
            let line_layout = if vi < visible_buffer_lines.len() && visible_buffer_lines[vi] == ix {
                let l = &lines[vi];
                vi += 1;
                Some(l)
            } else {
                None
            };

            if let Some(line) = line_layout {
                if cursor_pos.is_none() {
                    let offset = cursor.saturating_sub(prev_lines_offset);
                    if let Some(pos) =
                        line.position_for_index(offset, last_layout, state.cursor_line_end_affinity)
                    {
                        cursor_pos = Some(line_origin + pos);
                    }
                }
                if cursor_start.is_none() {
                    let offset = selected_range.start.saturating_sub(prev_lines_offset);
                    if let Some(pos) = line.position_for_index(offset, last_layout, false) {
                        cursor_start = Some(line_origin + pos);
                    }
                }
                if cursor_end.is_none() {
                    let offset = selected_range.end.saturating_sub(prev_lines_offset);
                    if let Some(pos) = line.position_for_index(offset, last_layout, false) {
                        cursor_end = Some(line_origin + pos);
                    }
                }

                offset_y += line.size(line_height).height;
                // +1 for the last `\n`
                prev_lines_offset += wrap_line.len() + 1;
            } else {
                // Not visible (before visible range or hidden/folded).
                // Just increase the offset_y and prev_lines_offset for scroll tracking.
                if prev_lines_offset >= cursor && cursor_pos.is_none() {
                    cursor_pos = Some(line_origin);
                }
                if prev_lines_offset >= selected_range.start && cursor_start.is_none() {
                    cursor_start = Some(line_origin);
                }
                if prev_lines_offset >= selected_range.end && cursor_end.is_none() {
                    cursor_end = Some(line_origin);
                }

                let visible_wrap_rows =
                    state.display_map.visible_wrap_row_count_for_buffer_line(ix);
                offset_y += line_height * visible_wrap_rows;
                // +1 for the last `\n`
                prev_lines_offset += wrap_line.len() + 1;
            }
        }

        if let (Some(cursor_pos), Some(cursor_start), Some(cursor_end)) =
            (cursor_pos, cursor_start, cursor_end)
        {
            let selection_changed = state.last_selected_range != Some(selected_range);

            if selection_changed && !is_selected_all {
                // For Right alignment use 0 margin: cursor is clamped to bounds separately,
                // so we never scroll the text for cursor-at-edge, avoiding a first-click jump.
                let safety_margin = match last_layout.text_align {
                    TextAlign::Left => RIGHT_MARGIN,
                    TextAlign::Right => px(0.),
                    TextAlign::Center => CURSOR_WIDTH,
                };

                scroll_offset.x = if scroll_offset.x + cursor_pos.x
                    > (bounds.size.width - line_number_width - safety_margin)
                {
                    // cursor is out of right
                    bounds.size.width - line_number_width - safety_margin - cursor_pos.x
                } else if scroll_offset.x + cursor_pos.x < px(0.) {
                    // cursor is out of left
                    scroll_offset.x - cursor_pos.x
                } else {
                    scroll_offset.x
                };

                // If we change the scroll_offset.y, GPUI will render and trigger the next run loop.
                // So, here we just adjust offset by `line_height` for move smooth.
                scroll_offset.y =
                    if scroll_offset.y + cursor_pos.y > bounds.size.height - top_bottom_margin {
                        // cursor is out of bottom
                        scroll_offset.y - line_height
                    } else if scroll_offset.y + cursor_pos.y < top_bottom_margin {
                        // cursor is out of top
                        (scroll_offset.y + line_height).min(px(0.))
                    } else {
                        scroll_offset.y
                    };

                // For selection to move scroll
                if state.selection_reversed {
                    if scroll_offset.x + cursor_start.x < px(0.) {
                        // selection start is out of left
                        scroll_offset.x = -cursor_start.x;
                    }
                    if scroll_offset.y + cursor_start.y < px(0.) {
                        // selection start is out of top
                        scroll_offset.y = -cursor_start.y;
                    }
                } else {
                    // TODO: Consider to remove this part,
                    // maybe is not necessary (But selection_reversed is needed).
                    if scroll_offset.x + cursor_end.x <= px(0.) {
                        // selection end is out of left
                        scroll_offset.x = -cursor_end.x;
                    }
                    if scroll_offset.y + cursor_end.y <= px(0.) {
                        // selection end is out of top
                        scroll_offset.y = -cursor_end.y;
                    }
                }
            }

            // cursor bounds
            let cursor_height = match state.size {
                crate::Size::Large => 1.,
                crate::Size::Small => 0.75,
                _ => 0.85,
            } * line_height;

            // For Right alignment, clamp cursor within the right edge of bounds so it
            // stays visible without having to shift the text via scroll_offset.
            let cursor_x = bounds.left() + cursor_pos.x + line_number_width + scroll_offset.x;
            let cursor_x = if last_layout.text_align == TextAlign::Right {
                cursor_x.min(bounds.right() - CURSOR_WIDTH)
            } else {
                cursor_x
            };
            cursor_bounds = Some(Bounds::new(
                point(
                    cursor_x,
                    bounds.top() + cursor_pos.y + ((line_height - cursor_height) / 2.),
                ),
                size(CURSOR_WIDTH, cursor_height),
            ));
        }

        if let Some(deferred_scroll_offset) = state.deferred_scroll_offset {
            scroll_offset = deferred_scroll_offset;
        }
        scroll_offset.y = clamp_auto_grow_vertical_scroll_offset(
            &state.mode,
            scroll_offset.y,
            scroll_size.height,
            bounds.size.height,
        );

        bounds.origin += scroll_offset;

        (cursor_bounds, scroll_offset)
    }

    /// Layout the match range to a Path.
    pub(crate) fn layout_match_range(
        range: Range<usize>,
        last_layout: &LastLayout,
        bounds: &Bounds<Pixels>,
    ) -> Option<Path<Pixels>> {
        if range.is_empty() {
            return None;
        }

        if range.start < last_layout.visible_range_offset.start
            || range.end > last_layout.visible_range_offset.end
        {
            return None;
        }

        let line_height = last_layout.line_height;
        let visible_top = last_layout.visible_top;
        let lines = &last_layout.lines;
        let line_number_width = last_layout.line_number_width;

        let start_ix = range.start;
        let end_ix = range.end;

        // Start from visible_top (which already accounts for all lines before visible range)
        let mut offset_y = visible_top;
        let mut line_corners = vec![];

        // Iterate only over visible (non-hidden) buffer lines
        for (prev_lines_offset, line) in last_layout
            .visible_line_byte_offsets
            .iter()
            .zip(lines.iter())
        {
            let prev_lines_offset = *prev_lines_offset;
            let line_size = line.size(line_height);
            let line_wrap_width = line_size.width;

            let line_origin = point(px(0.), offset_y);

            let line_cursor_start = line.position_for_index(
                start_ix.saturating_sub(prev_lines_offset),
                last_layout,
                false,
            );
            let line_cursor_end = line.position_for_index(
                end_ix.saturating_sub(prev_lines_offset),
                last_layout,
                false,
            );

            if line_cursor_start.is_some() || line_cursor_end.is_some() {
                let start = line_cursor_start
                    .unwrap_or_else(|| line.position_for_index(0, last_layout, false).unwrap());

                let end = line_cursor_end.unwrap_or_else(|| {
                    line.position_for_index(line.len(), last_layout, false)
                        .unwrap()
                });

                // Split the selection into multiple items
                let wrapped_lines =
                    (end.y / line_height).ceil() as usize - (start.y / line_height).ceil() as usize;

                let mut end_x = end.x;
                if wrapped_lines > 0 {
                    end_x = line_wrap_width;
                }

                // Ensure at least 6px width for the selection for empty lines.
                end_x = end_x.max(start.x + px(6.));

                line_corners.push(Corners {
                    top_left: line_origin + point(start.x, start.y),
                    top_right: line_origin + point(end_x, start.y),
                    bottom_left: line_origin + point(start.x, start.y + line_height),
                    bottom_right: line_origin + point(end_x, start.y + line_height),
                });

                // wrapped lines
                for i in 1..=wrapped_lines {
                    let start = point(px(0.), start.y + i as f32 * line_height);
                    let mut end = point(end.x, end.y + i as f32 * line_height);
                    if i < wrapped_lines {
                        end.x = line_size.width;
                    }

                    line_corners.push(Corners {
                        top_left: line_origin + point(start.x, start.y),
                        top_right: line_origin + point(end.x, start.y),
                        bottom_left: line_origin + point(start.x, start.y + line_height),
                        bottom_right: line_origin + point(end.x, start.y + line_height),
                    });
                }
            }

            if line_cursor_start.is_some() && line_cursor_end.is_some() {
                break;
            }

            offset_y += line_size.height;
        }

        let mut points = vec![];
        if line_corners.is_empty() {
            return None;
        }

        // Fix corners to make sure the left to right direction
        for corners in &mut line_corners {
            if corners.top_left.x > corners.top_right.x {
                std::mem::swap(&mut corners.top_left, &mut corners.top_right);
                std::mem::swap(&mut corners.bottom_left, &mut corners.bottom_right);
            }
        }

        for corners in &line_corners {
            points.push(corners.top_right);
            points.push(corners.bottom_right);
            points.push(corners.bottom_left);
        }

        let mut rev_line_corners = line_corners.iter().rev().peekable();
        while let Some(corners) = rev_line_corners.next() {
            points.push(corners.top_left);
            if let Some(next) = rev_line_corners.peek()
                && next.top_left.x > corners.top_left.x
            {
                points.push(point(next.top_left.x, corners.top_left.y));
            }
        }

        // print_points_as_svg_path(&line_corners, &points);

        let path_origin = bounds.origin + point(line_number_width, px(0.));
        let first_p = *points.first().unwrap();
        let mut builder = gpui::PathBuilder::fill();
        builder.move_to(path_origin + first_p);
        for p in points.iter().skip(1) {
            builder.line_to(path_origin + *p);
        }

        builder.build().ok()
    }

    fn layout_selections(
        &self,
        last_layout: &LastLayout,
        bounds: &mut Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Path<Pixels>> {
        let state = self.state.read(cx);
        if !state.focus_handle.is_focused(window) {
            return None;
        }

        let mut selected_range = state.selected_range;
        if let Some(ime_marked_range) = &state.ime_marked_range
            && !ime_marked_range.is_empty()
        {
            selected_range = (ime_marked_range.end..ime_marked_range.end).into();
        }
        if selected_range.is_empty() {
            return None;
        }

        if state.masked {
            selected_range.start = masked_display_offset(&state.text, selected_range.start);
            selected_range.end = masked_display_offset(&state.text, selected_range.end);
        }

        let (start_ix, end_ix) = if selected_range.start < selected_range.end {
            (selected_range.start, selected_range.end)
        } else {
            (selected_range.end, selected_range.start)
        };

        let range = start_ix.max(last_layout.visible_range_offset.start)
            ..end_ix.min(last_layout.visible_range_offset.end);

        Self::layout_match_range(range, last_layout, bounds)
    }

    /// Calculate the visible range of lines in the viewport.
    ///
    /// Returns
    ///
    /// - visible_range: The visible range is based on unwrapped lines (Zero based).
    /// - visible_buffer_lines: Indices of non-hidden buffer lines within the visible range.
    /// - visible_top: The top position of the first visible line in the scroll viewport.
    fn calculate_visible_range(
        &self,
        state: &InputState,
        line_height: Pixels,
        input_height: Pixels,
    ) -> (Range<usize>, Vec<usize>, Pixels) {
        // Add extra rows to avoid showing empty space when scroll to bottom.
        let extra_rows = 1;
        let mut visible_top = px(0.);
        if state.mode.is_single_line() {
            return (0..1, vec![0], visible_top);
        }

        let total_lines = state.display_map.wrap_row_count();
        let mut scroll_top = if let Some(deferred_scroll_offset) = state.deferred_scroll_offset {
            deferred_scroll_offset.y
        } else {
            state.scroll_handle.offset().y
        };

        let mut visible_range = 0..total_lines;
        scroll_top = clamp_auto_grow_vertical_scroll_offset(
            &state.mode,
            scroll_top,
            line_height * total_lines,
            input_height,
        );
        let mut line_bottom = px(0.);
        for (ix, _line) in state.display_map.lines().iter().enumerate() {
            let visible_wrap_rows = state.display_map.visible_wrap_row_count_for_buffer_line(ix);

            if visible_wrap_rows == 0 {
                continue;
            }

            let wrapped_height = line_height * visible_wrap_rows;
            line_bottom += wrapped_height;

            if line_bottom < -scroll_top {
                visible_top = line_bottom - wrapped_height;
                visible_range.start = ix;
            }

            if line_bottom + scroll_top >= input_height {
                visible_range.end = (ix + extra_rows).min(total_lines);
                break;
            }
        }

        // Collect non-hidden buffer lines within the visible range
        let mut visible_buffer_lines = Vec::with_capacity(visible_range.len());
        for ix in visible_range.start..visible_range.end {
            let visible_wrap_rows = state.display_map.visible_wrap_row_count_for_buffer_line(ix);
            if visible_wrap_rows > 0 {
                visible_buffer_lines.push(ix);
            }
        }

        (visible_range, visible_buffer_lines, visible_top)
    }

    /// Layout shaped lines for whitespace indicators (space and tab).
    ///
    /// Returns `WhitespaceIndicators` with shaped lines for space and tab characters.
    fn layout_whitespace_indicators(
        _state: &InputState,
        text_size: Pixels,
        style: &TextStyle,
        window: &mut Window,
        cx: &App,
    ) -> Option<WhitespaceIndicators> {
        // Whitespace indicators are not currently enabled.
        // When re-enabled, check `state.show_whitespaces` to conditionally enable.
        let _ = (text_size, style, window, cx);
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_lines(
        state: &InputState,
        display_text: &Rope,
        last_layout: &LastLayout,
        font_size: Pixels,
        runs: &[TextRun],
        bg_segments: &[(Range<usize>, Hsla)],
        whitespace_indicators: Option<WhitespaceIndicators>,
        window: &mut Window,
    ) -> Vec<LineLayout> {
        let is_single_line = state.mode.is_single_line();
        let buffer_lines = state.display_map.lines();

        if is_single_line {
            let shaped_line = window.text_system().shape_line(
                display_text.to_string().into(),
                font_size,
                runs,
                None,
            );

            let line_layout = LineLayout::new()
                .lines(smallvec::smallvec![shaped_line])
                .with_whitespaces(whitespace_indicators);
            return vec![line_layout];
        }

        // Empty to use placeholder, the placeholder is not in the wrapper map.
        if state.text.len() == 0 {
            let placeholder_text = display_text.to_string();
            let mut placeholder_lines = SmallVec::new();

            for (line, line_runs) in placeholder_line_runs(&placeholder_text, runs) {
                let shaped_line = window.text_system().shape_line(
                    line.to_string().into(),
                    font_size,
                    &line_runs,
                    None,
                );
                placeholder_lines.push(shaped_line);
            }

            // Keep placeholder lines in a single layout to stay parallel with visible_* metadata.
            let line_layout = LineLayout::new()
                .lines(placeholder_lines)
                .with_whitespaces(whitespace_indicators);
            return vec![line_layout];
        }

        let mut lines = Vec::with_capacity(last_layout.visible_buffer_lines.len());

        for (vi, &buffer_line) in last_layout.visible_buffer_lines.iter().enumerate() {
            let line_text: String = display_text.slice_line(buffer_line).into();
            let line_item = buffer_lines
                .get(buffer_line)
                .expect("line should exists in wrapper");

            debug_assert_eq!(line_item.len(), line_text.len());

            let mut wrapped_lines = SmallVec::with_capacity(1);
            let line_offset = display_text.line_start_offset(buffer_line);

            for range in &line_item.wrapped_lines {
                let line_runs = runs_for_range(runs, line_offset, range);
                let line_runs = if bg_segments.is_empty() {
                    line_runs
                } else {
                    split_runs_by_bg_segments(
                        last_layout.visible_line_byte_offsets[vi] + (range.start),
                        &line_runs,
                        bg_segments,
                    )
                };

                let sub_line: SharedString = line_text[range.clone()].to_string().into();
                let shaped_line = window
                    .text_system()
                    .shape_line(sub_line, font_size, &line_runs, None);

                wrapped_lines.push(shaped_line);
            }

            let line_layout = LineLayout::new()
                .lines(wrapped_lines)
                .with_whitespaces(whitespace_indicators.clone());
            lines.push(line_layout);
        }

        lines
    }
}

pub(super) struct PrepaintState {
    /// The lines of entire lines.
    last_layout: LastLayout,
    /// Size of the scrollable area by entire lines.
    scroll_size: Size<Pixels>,
    cursor_bounds: Option<Bounds<Pixels>>,
    cursor_scroll_offset: Point<Pixels>,
    selection_path: Option<Path<Pixels>>,
    bounds: Bounds<Pixels>,
}

impl PrepaintState {
    /// Returns cursor bounds adjusted for scroll offset, if available.
    fn cursor_bounds_with_scroll(&self) -> Option<Bounds<Pixels>> {
        self.cursor_bounds.map(|mut bounds| {
            bounds.origin.y += self.cursor_scroll_offset.y;
            bounds
        })
    }
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// A debug function to print points as SVG path.
#[allow(unused)]
fn print_points_as_svg_path(line_corners: &Vec<gpui::Corners<Pixels>>, points: Vec<Point<Pixels>>) {
    for corners in line_corners {
        println!(
            "tl: ({}, {}), tr: ({}, {}), bl: ({}, {}), br: ({}, {})",
            corners.top_left.as_f32() as i32,
            corners.top_left.as_f32() as i32,
            corners.top_right.as_f32() as i32,
            corners.top_right.as_f32() as i32,
            corners.bottom_left.as_f32() as i32,
            corners.bottom_left.as_f32() as i32,
            corners.bottom_right.as_f32() as i32,
            corners.bottom_right.as_f32() as i32,
        );
    }

    if !points.is_empty() {
        println!(
            "M{},{}",
            points[0].x.as_f32() as i32,
            points[0].y.as_f32() as i32
        );
        for p in points.iter().skip(1) {
            println!("L{},{}", p.x.as_f32() as i32, p.y.as_f32() as i32);
        }
    }
}
impl Element for TextElement {
    type PrepaintState = PrepaintState;
    type RequestLayoutState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = self.state.read(cx);
        let line_height = window.line_height();

        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if state.mode.is_multi_line() {
            style.flex_grow = 1.0;
            style.size.height = relative(1.).into();
            if state.mode.is_auto_grow() {
                // Auto grow to let height match to rows, but not exceed max rows.
                let rows = state.mode.max_rows().min(state.mode.rows());
                style.min_size.height = (rows * line_height).into();
            } else {
                style.min_size.height = line_height.into();
            }
        } else {
            // For single-line inputs, the minimum height should be the line height
            style.size.height = line_height.into();
        };

        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        let font = style.font();
        let text_size = style.font_size.to_pixels(window.rem_size());

        self.state.update(cx, |state, cx| {
            state.display_map.set_font(font, text_size, cx);
            state.display_map.ensure_text_prepared(&state.text, cx);
        });

        let state = self.state.read(cx);
        let line_height = window.line_height();

        let (visible_range, visible_buffer_lines, visible_top) =
            self.calculate_visible_range(state, line_height, bounds.size.height);
        let visible_start_offset = state.text.line_start_offset(visible_range.start);
        let visible_end_offset = state
            .text
            .line_end_offset(visible_range.end.saturating_sub(1));

        let state = self.state.read(cx);
        let multi_line = state.mode.is_multi_line();
        let text = state.text.clone();
        let is_empty = text.len() == 0;
        let placeholder = self.placeholder.clone();

        let text_style = window.text_style();
        let fg = text_style.color;
        let (display_text, text_color) = if is_empty {
            (&Rope::from(placeholder.as_str()), cx.theme().text_muted)
        } else if state.masked {
            (
                &Rope::from(MASK_CHAR.to_string().repeat(text.chars().count())),
                fg,
            )
        } else {
            (&text, fg)
        };

        // Line numbers are not used (code editor mode removed)
        let line_number_width = px(0.);

        let mut bounds = bounds;
        let wrap_width = if multi_line && state.soft_wrap {
            Some(bounds.size.width - line_number_width - RIGHT_MARGIN)
        } else {
            None
        };

        let visible_line_byte_offsets: Vec<usize> = visible_buffer_lines
            .iter()
            .map(|&bl| state.text.line_start_offset(bl))
            .collect();

        // For password input (masked: true), convert byte offsets to masked display byte offsets so that
        // layout_match_range and position_for_index work in the correct coordinate space.
        let (visible_line_byte_offsets, visible_range_offset) = if state.masked {
            let offsets = visible_line_byte_offsets
                .iter()
                .map(|&o| masked_display_offset(&text, o))
                .collect();
            let range_offset = masked_display_offset(&text, visible_start_offset)
                ..masked_display_offset(&text, visible_end_offset);
            (offsets, range_offset)
        } else {
            (
                visible_line_byte_offsets,
                visible_start_offset..visible_end_offset,
            )
        };

        let mut last_layout = LastLayout {
            visible_range,
            visible_buffer_lines,
            visible_line_byte_offsets,
            visible_top,
            visible_range_offset,
            line_height,
            wrap_width,
            line_number_width,
            lines: Rc::new(vec![]),
            cursor_bounds: None,
            text_align: state.text_align,
            content_width: bounds.size.width,
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let marked_run = TextRun {
            len: 0,
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(text_color),
                wavy: false,
            }),
            strikethrough: None,
        };

        let runs = if !is_empty {
            vec![run]
        } else if let Some(ime_marked_range) = &state.ime_marked_range {
            // IME marked text
            vec![
                TextRun {
                    len: ime_marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: ime_marked_range.end - ime_marked_range.start,
                    underline: marked_run.underline,
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - ime_marked_range.end,
                    ..run.clone()
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        // Create shaped lines for whitespace indicators before layout
        let whitespace_indicators =
            Self::layout_whitespace_indicators(state, text_size, &text_style, window, cx);

        let lines = Self::layout_lines(
            state,
            display_text,
            &last_layout,
            text_size,
            &runs,
            &[],
            whitespace_indicators,
            window,
        );

        let mut longest_line_width = wrap_width.unwrap_or(px(0.));
        // 1. Single line
        // 2. Multi-line with soft wrap disabled.
        if state.mode.is_single_line() || !state.soft_wrap {
            let longest_row = state.display_map.longest_row();
            let longest_line: SharedString = state.text.slice_line(longest_row).to_string().into();
            longest_line_width = window
                .text_system()
                .shape_line(
                    longest_line.clone(),
                    text_size,
                    &[TextRun {
                        len: longest_line.len(),
                        font: style.font(),
                        color: gpui::black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    wrap_width,
                )
                .width;
        }
        last_layout.lines = Rc::new(lines);

        let total_wrapped_lines = state.display_map.wrap_row_count();
        let empty_bottom_height = px(0.);

        let mut scroll_size = size(
            if longest_line_width + line_number_width + RIGHT_MARGIN > bounds.size.width {
                longest_line_width + line_number_width + RIGHT_MARGIN
            } else {
                longest_line_width
            },
            (total_wrapped_lines as f32 * line_height + empty_bottom_height)
                .max(bounds.size.height),
        );

        // TODO: should be add some gap to right, to convenient to focus on boundary position
        if last_layout.text_align == TextAlign::Right || last_layout.text_align == TextAlign::Center
        {
            scroll_size.width = longest_line_width + line_number_width;
        }

        // `position_for_index` for example
        //
        // #### text
        //
        // Hello 世界，this is GPUI component.
        // The GPUI Component is a collection of UI components for
        // GPUI framework, including Button, Input, Checkbox, Radio,
        // Dropdown, Tab, and more...
        //
        // wrap_width: 444px, line_height: 20px
        //
        // #### lines[0]
        //
        // | index | pos              | line |
        // |-------|------------------|------|
        // | 5     | (37 px, 0.0)     | 0    |
        // | 38    | (261.7 px, 20.0) | 0    |
        // | 40    | None             | -    |
        //
        // #### lines[1]
        //
        // | index | position              | line |
        // |-------|-----------------------|------|
        // | 5     | (43.578125 px, 0.0)   | 0    |
        // | 56    | (422.21094 px, 0.0)   | 0    |
        // | 57    | (11.6328125 px, 20.0) | 1    |
        // | 114   | (429.85938 px, 20.0)  | 1    |
        // | 115   | (11.3125 px, 40.0)    | 2    |

        // Calculate the scroll offset to keep the cursor in view

        // Save the bounds before layout_cursor modifies bounds.origin with scroll_offset.
        let input_bounds = bounds;

        let (cursor_bounds, cursor_scroll_offset) =
            self.layout_cursor(&last_layout, &mut bounds, scroll_size, window, cx);
        last_layout.cursor_bounds = cursor_bounds;

        let selection_path = self.layout_selections(&last_layout, &mut bounds, window, cx);

        let state = self.state.read(cx);

        state
            .editor_scrollbar_snapshot
            .set(Some(EditorScrollbarSnapshot::new(
                input_bounds,
                &last_layout,
                scroll_size,
                cursor_scroll_offset,
                state,
            )));

        PrepaintState {
            bounds,
            last_layout,
            scroll_size,
            cursor_bounds,
            cursor_scroll_offset,
            selection_path,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        input_bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.state.read(cx).focus_handle.clone();
        let show_cursor = self.state.read(cx).show_cursor(window, cx);
        let focused = focus_handle.is_focused(window);
        let bounds = prepaint.bounds;
        let selected_range = self.state.read(cx).selected_range;
        let text_align = prepaint.last_layout.text_align;

        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.state.clone()),
            cx,
        );

        // Set Root focused_input when self is focused
        if focused {
            let state = self.state.clone();
            if Root::read(window, cx).focused_input.as_ref() != Some(&state) {
                Root::update(window, cx, |root, _, cx| {
                    root.focused_input = Some(state);
                    cx.notify();
                });
            }
        }

        // And reset focused_input when next_frame start
        window.on_next_frame({
            let state = self.state.clone();
            move |window, cx| {
                if !focused && Root::read(window, cx).focused_input.as_ref() == Some(&state) {
                    Root::update(window, cx, |root, _, cx| {
                        root.focused_input = None;
                        cx.notify();
                    });
                }
            }
        });

        // Paint multi line text
        let line_height = window.line_height();
        let origin = bounds.origin;

        let invisible_top_padding = prepaint.last_layout.visible_top;

        // Paint selections
        if window.is_window_active()
            && let Some(path) = prepaint.selection_path.take()
        {
            window.paint_path(path, cx.theme().selection);
        }

        // Paint text
        let mut offset_y = invisible_top_padding;

        // Keep scrollbar offset always be positive，Start from the left position
        let scroll_offset = if text_align == TextAlign::Right {
            (prepaint.scroll_size.width - prepaint.bounds.size.width).max(px(0.))
        } else if text_align == TextAlign::Center {
            (prepaint.scroll_size.width - prepaint.bounds.size.width)
                .half()
                .max(px(0.))
        } else {
            px(0.)
        };

        for (line, _buffer_line) in prepaint
            .last_layout
            .lines
            .iter()
            .zip(prepaint.last_layout.visible_buffer_lines.iter())
        {
            let line_y = origin.y + offset_y;
            let p = point(
                origin.x + prepaint.last_layout.line_number_width + (scroll_offset),
                line_y,
            );

            // Paint the actual line
            line.paint(
                p,
                line_height,
                text_align,
                Some(prepaint.last_layout.content_width),
                window,
                cx,
            );
            offset_y += line.size(line_height).height;
        }

        // Paint blinking cursor
        if focused
            && show_cursor
            && let Some(cursor_bounds) = prepaint.cursor_bounds_with_scroll()
        {
            window.paint_quad(fill(cursor_bounds, cx.theme().cursor));
        }

        self.state.update(cx, |state, cx| {
            state.last_layout = Some(prepaint.last_layout.clone());
            state.last_bounds = Some(bounds);
            state.last_cursor = Some(state.cursor());
            state.set_input_bounds(input_bounds, cx);
            state.last_selected_range = Some(selected_range);
            state.scroll_size = prepaint.scroll_size;
            state.update_scroll_offset(Some(prepaint.cursor_scroll_offset), cx);
            state.deferred_scroll_offset = None;

            cx.notify();
        });

        self.paint_mouse_listeners(window, cx);
    }
}

/// Split placeholder text into display lines and trim runs to each line.
fn placeholder_line_runs<'a>(
    display_text: &'a str,
    runs: &[TextRun],
) -> Vec<(&'a str, Vec<TextRun>)> {
    let mut result = Vec::new();
    let mut line_offset = 0;

    for line in display_text.split('\n') {
        let line_runs = runs_for_range(runs, line_offset, &(0..line.len()));
        debug_assert_eq!(
            line_runs.iter().map(|run| run.len).sum::<usize>(),
            line.len()
        );
        result.push((line, line_runs));
        // Advance in the whole-placeholder coordinate space, including the separator.
        line_offset += line.len() + 1;
    }

    result
}

/// Get the runs for the given range.
///
/// The range is the byte range of the wrapped line.
pub(super) fn runs_for_range(
    runs: &[TextRun],
    line_offset: usize,
    range: &Range<usize>,
) -> Vec<TextRun> {
    let mut result = vec![];
    let range = (line_offset + range.start)..(line_offset + range.end);
    let mut cursor = 0;

    for run in runs {
        let run_start = cursor;
        let run_end = cursor + run.len;

        if run_end <= range.start {
            cursor = run_end;
            continue;
        }

        if run_start >= range.end {
            break;
        }

        let start = range.start.max(run_start) - run_start;
        let end = range.end.min(run_end) - run_start;
        let len = end - start;

        if len > 0 {
            result.push(TextRun { len, ..run.clone() });
        }

        cursor = run_end;
    }

    result
}

fn split_runs_by_bg_segments(
    start_offset: usize,
    runs: &[TextRun],
    bg_segments: &[(Range<usize>, Hsla)],
) -> Vec<TextRun> {
    let mut result = vec![];

    let mut cursor = start_offset;
    for run in runs {
        let mut run_start = cursor;
        let run_end = cursor + run.len;

        for (bg_range, bg_color) in bg_segments {
            if run_end <= bg_range.start || run_start >= bg_range.end {
                continue;
            }

            // Overlap exists
            if run_start < bg_range.start {
                // Add the part before the background range
                result.push(TextRun {
                    len: bg_range.start - run_start,
                    ..run.clone()
                });
            }

            // Add the overlapping part with background color
            let overlap_start = run_start.max(bg_range.start);
            let overlap_end = run_end.min(bg_range.end);
            let text_color = if bg_color.l >= 0.5 {
                gpui::black()
            } else {
                gpui::white()
            };

            let run_len = overlap_end.saturating_sub(overlap_start);
            if run_len > 0 {
                result.push(TextRun {
                    len: run_len,
                    color: text_color,
                    ..run.clone()
                });

                cursor = bg_range.end;
                run_start = cursor;
            }
        }

        if run_end > cursor {
            // Add the part after the background range
            result.push(TextRun {
                len: run_end - cursor,
                ..run.clone()
            });
        }

        cursor = run_end;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_editor_scrollbar_layout_uses_current_scroll_size() {
        let input_bounds = Bounds::new(point(px(10.), px(20.)), size(px(300.), px(80.)));
        let paddings = Edges {
            top: px(2.),
            right: px(3.),
            bottom: px(5.),
            left: px(7.),
        };

        let layout =
            EditorScrollbarLayout::new(input_bounds, px(40.), size(px(1000.), px(200.)), paddings);

        assert_eq!(
            layout.bounds,
            Bounds::new(point(px(57.), px(18.)), size(px(256.), px(87.)))
        );
        assert_eq!(layout.scroll_size, size(px(966.), px(200.)));

        let layout_without_gutter =
            EditorScrollbarLayout::new(input_bounds, px(0.), size(px(500.), px(120.)), paddings);

        assert_eq!(
            layout_without_gutter.bounds,
            Bounds::new(point(px(10.), px(18.)), size(px(303.), px(87.)))
        );
        assert_eq!(layout_without_gutter.scroll_size, size(px(513.), px(120.)));
    }

    #[test]
    fn test_auto_grow_scroll_offset_is_clamped_to_current_viewport() {
        let mode = InputMode::auto_grow(3, 8);

        assert_eq!(
            clamp_auto_grow_vertical_scroll_offset(&mode, px(-260.), px(340.), px(160.)),
            px(-180.)
        );
        assert_eq!(
            clamp_auto_grow_vertical_scroll_offset(&mode, px(-40.), px(340.), px(160.)),
            px(-40.)
        );
        assert_eq!(
            clamp_auto_grow_vertical_scroll_offset(&mode, px(20.), px(340.), px(160.)),
            px(0.)
        );

        let plain_text = InputMode::plain_text().multi_line(true);
        assert_eq!(
            clamp_auto_grow_vertical_scroll_offset(&plain_text, px(-260.), px(340.), px(160.)),
            px(-260.)
        );
    }

    #[test]
    fn test_runs_for_range() {
        let run = TextRun {
            len: 0,
            font: gpui::font(".SystemUIFont"),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        // use hello this-is-test
        let runs = vec![
            // use
            TextRun {
                len: 3,
                ..run.clone()
            },
            // \s
            TextRun {
                len: 1,
                ..run.clone()
            },
            // hello
            TextRun {
                len: 5,
                ..run.clone()
            },
            // \s
            TextRun {
                len: 1,
                ..run.clone()
            },
            // this-is-test
            TextRun {
                len: 12,
                ..run.clone()
            },
        ];

        #[track_caller]
        fn assert_runs(actual: Vec<TextRun>, expected: &[usize]) {
            let left = actual.iter().map(|run| run.len).collect::<Vec<_>>();
            assert_eq!(left, expected);
        }

        assert_runs(runs_for_range(&runs, 0, &(0..0)), &[]);
        assert_runs(runs_for_range(&runs, 0, &(0..100)), &[3, 1, 5, 1, 12]);

        assert_runs(runs_for_range(&runs, 0, &(0..6)), &[3, 1, 2]);
        assert_runs(runs_for_range(&runs, 0, &(1..6)), &[2, 1, 2]);
        assert_runs(runs_for_range(&runs, 0, &(3..10)), &[1, 5, 1]);
        assert_runs(runs_for_range(&runs, 0, &(5..8)), &[3]);
        assert_runs(runs_for_range(&runs, 3, &(0..3)), &[1, 2]);
        assert_runs(runs_for_range(&runs, 3, &(2..10)), &[4, 1, 3]);
        assert_runs(runs_for_range(&runs, 9, &(0..8)), &[1, 7]);
    }

    #[test]
    fn test_placeholder_line_runs() {
        let run = TextRun {
            len: 0,
            font: gpui::font(".SystemUIFont"),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let runs = vec![
            TextRun {
                len: 2,
                ..run.clone()
            },
            TextRun {
                len: 2,
                ..run.clone()
            },
            TextRun { len: 1, ..run },
        ];

        let placeholder_runs = placeholder_line_runs("ab\n\nc", &runs);

        let lines = placeholder_runs
            .iter()
            .map(|(line, _)| *line)
            .collect::<Vec<_>>();
        assert_eq!(lines, vec!["ab", "", "c"]);

        let run_lengths = placeholder_runs
            .iter()
            .map(|(_, line_runs)| line_runs.iter().map(|run| run.len).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        assert_eq!(run_lengths, vec![vec![2], vec![], vec![1]]);
    }

    #[test]
    fn test_split_runs_by_bg_segments() {
        let run = TextRun {
            len: 0,
            font: gpui::font(".SystemUIFont"),
            color: gpui::blue(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let runs = vec![
            TextRun {
                len: 5,
                ..run.clone()
            },
            TextRun {
                len: 7,
                ..run.clone()
            },
            TextRun {
                len: 24,
                ..run.clone()
            },
        ];

        let bg_segments = vec![(8..12, gpui::red()), (12..18, gpui::blue())];
        let result = split_runs_by_bg_segments(5, &runs, &bg_segments);
        assert_eq!(
            result.iter().map(|run| run.len).collect::<Vec<_>>(),
            vec![3, 2, 2, 5, 1, 23]
        );
        assert_eq!(result[0].color, gpui::blue());
        assert_eq!(result[1].color, gpui::black());
        assert_eq!(result[2].color, gpui::black());
        assert_eq!(result[3].color, gpui::black());
        assert_eq!(result[4].color, gpui::black());
        assert_eq!(result[5].color, gpui::blue());
    }
}
