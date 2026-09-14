use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, App, AppContext, Context, DefiniteLength, DismissEvent, DragMoveEvent, Empty, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window, div, px, rems,
};
use theme::{ActiveTheme, AxisExt, Placement, TABBAR_HEIGHT};

use crate::button::{Button, ButtonVariants as _};
use crate::dock::dock::DockPlacement;
use crate::dock::panel::{Panel, PanelView};
use crate::dock::stack_panel::StackPanel;
use crate::dock::{
    CloseAllPanels, ClosePanel, DockArea, NextPanel, PanelEvent, PanelStyle, PreviousPanel,
    ReopenClosedPanel,
};
use crate::menu::{DropdownMenu, PopupMenu};
use crate::tab::Tab;
use crate::tab::tab_bar::TabBar;
use crate::{IconName, Selectable, Sizable, StyledExt, h_flex, v_flex};

#[derive(Clone)]
struct TabState {
    closable: bool,
    draggable: bool,
    droppable: bool,
    active_panel: Option<Arc<dyn PanelView>>,
}

#[derive(Clone)]
pub(crate) struct DragPanel {
    pub(crate) panel: Arc<dyn PanelView>,
    pub(crate) tab_panel: Entity<TabPanel>,
}

impl DragPanel {
    pub(crate) fn new(panel: Arc<dyn PanelView>, tab_panel: Entity<TabPanel>) -> Self {
        Self { panel, tab_panel }
    }
}

impl Render for DragPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("drag-panel")
            .cursor_grab()
            .p_2()
            .min_w_24()
            .justify_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .rounded(cx.theme().radius)
            .text_sm()
            .text_color(cx.theme().text)
            .text_ellipsis()
            .when(cx.theme().shadow, |this| this.shadow_xs())
            .bg(cx.theme().background)
            .child(self.panel.title(cx))
    }
}

pub struct TabPanel {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,

    /// List of panels in the tab panel
    pub(crate) panels: Vec<Arc<dyn PanelView>>,

    /// Current active panel index
    pub(crate) active_ix: usize,

    /// If this is true, the Panel closeable will follow the active panel's closeable,
    /// otherwise this TabPanel will not able to close
    pub(crate) closable: bool,

    /// The stock_panel can be None, if is None, that means the panels can't be split or move
    stack_panel: Option<WeakEntity<StackPanel>>,

    /// Scroll handle for the tab bar
    tab_bar_scroll_handle: ScrollHandle,

    /// Whether the tab panel is zoomeds
    zoomed: bool,

    /// Whether the tab panel is collapsed
    collapsed: bool,

    /// When drag move, will get the placement of the panel to be split
    will_split_placement: Option<Placement>,
}

impl Panel for TabPanel {
    fn panel_id(&self) -> SharedString {
        "TabPanel".into()
    }

    fn title(&self, cx: &App) -> gpui::AnyElement {
        self.active_panel(cx)
            .map(|panel| panel.title(cx))
            .unwrap_or("Empty Tab".into_any_element())
    }

    fn closable(&self, cx: &App) -> bool {
        if !self.closable {
            return false;
        }

        self.active_panel(cx)
            .map(|panel| panel.closable(cx))
            .unwrap_or(false)
    }

    fn zoomable(&self, cx: &App) -> bool {
        self.active_panel(cx)
            .map(|panel| panel.zoomable(cx))
            .unwrap_or(false)
    }

    fn visible(&self, cx: &App) -> bool {
        self.panels.is_empty() || self.visible_panels(cx).next().is_some()
    }

    fn popup_menu(&self, menu: PopupMenu, cx: &App) -> PopupMenu {
        if let Some(panel) = self.active_panel(cx) {
            panel.popup_menu(menu, cx)
        } else {
            menu
        }
    }

    fn toolbar_buttons(&self, window: &Window, cx: &App) -> Vec<Button> {
        if let Some(panel) = self.active_panel(cx) {
            panel.toolbar_buttons(window, cx)
        } else {
            vec![]
        }
    }
}

impl TabPanel {
    pub fn new(
        stack_panel: Option<WeakEntity<StackPanel>>,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            stack_panel,
            panels: Vec::new(),
            active_ix: 0,
            tab_bar_scroll_handle: ScrollHandle::new(),
            will_split_placement: None,
            zoomed: false,
            collapsed: false,
            closable: true,
        }
    }

    pub(super) fn set_parent(&mut self, view: WeakEntity<StackPanel>) {
        self.stack_panel = Some(view);
    }

    /// Return current active_panel view
    pub fn active_panel(&self, cx: &App) -> Option<Arc<dyn PanelView>> {
        let panel = self.panels.get(self.active_ix);

        if let Some(panel) = panel {
            if panel.visible(cx) {
                Some(panel.clone())
            } else {
                // Return the first visible panel
                self.visible_panels(cx).next()
            }
        } else {
            None
        }
    }

    fn set_active_ix(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.active_ix {
            self.focus_active_panel(window, cx);
            return;
        }

        let last_active_ix = self.active_ix;

        self.active_ix = ix;
        self.tab_bar_scroll_handle.scroll_to_item(ix);
        self.focus_active_panel(window, cx);

        // Sync the active state to all panels
        cx.spawn(async move |view, cx| {
            view.update(cx, |view, cx| {
                if let Some(last_active) = view.panels.get(last_active_ix) {
                    last_active.set_active(false, cx);
                }
                if let Some(active) = view.panels.get(view.active_ix) {
                    active.set_active(true, cx);
                }
            })
            .ok();
        })
        .detach();

        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Add a panel to the end of the tabs
    pub fn add_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_panel_with_active(panel, true, window, cx);
    }

    fn add_panel_with_active(
        &mut self,
        panel: Arc<dyn PanelView>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .panels
            .iter()
            .any(|p| p.panel_id(cx) == panel.panel_id(cx))
        {
            // Set the active panel to the matched panel
            if active
                && let Some(ix) = self
                    .panels
                    .iter()
                    .position(|p| p.panel_id(cx) == panel.panel_id(cx))
            {
                self.set_active_ix(ix, window, cx);
            }

            return;
        }

        self.panels.push(panel);

        // Set the active panel to the new panel
        if active {
            self.set_active_ix(self.panels.len() - 1, window, cx);
        }

        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Add panel to try to split
    pub fn add_panel_at(
        &mut self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |view, cx| {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.will_split_placement = Some(placement);
                    view.split_panel(panel, placement, size, window, cx)
                })
                .ok()
            })
            .ok()
        })
        .detach();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    fn insert_panel_at(
        &mut self,
        panel: Arc<dyn PanelView>,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .panels
            .iter()
            .any(|p| p.view().entity_id() == panel.view().entity_id())
        {
            return;
        }

        self.panels.insert(ix, panel);
        self.set_active_ix(ix, window, cx);
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Remove a panel from the tab panel
    pub fn remove_panel(
        &mut self,
        panel: &Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.closable || !panel.closable(cx) || !self.panels.contains(panel) {
            return;
        }
        let tab = cx.entity().downgrade();
        _ = self
            .dock_area
            .update(cx, |dock, _| dock.remember_closed(tab, panel.clone()));
        self.detach_panel(panel, window, cx);
        self.remove_self_if_empty(window, cx);
        if self.panels.is_empty() {
            window.focus(&self.focus_handle, cx);
            let dock = self.dock_area.clone();
            window.defer(cx, move |window, cx| {
                if let Some(dock) = dock.upgrade() {
                    dock.update(cx, |dock, cx| dock.focus_tab_panel(window, cx));
                }
            });
        }

        cx.emit(PanelEvent::ZoomOut);
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    fn detach_panel(
        &mut self,
        panel: &Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel_view = panel.view();
        let removed_ix = self.panels.iter().position(|p| p.view() == panel_view);
        self.panels.retain(|p| p.view() != panel_view);

        if self.active_ix >= self.panels.len() {
            self.set_active_ix(self.panels.len().saturating_sub(1), window, cx)
        } else if let Some(removed_ix) = removed_ix {
            if removed_ix < self.active_ix {
                self.active_ix = self.active_ix.saturating_sub(1);
            } else if removed_ix == self.active_ix {
                // The active panel was removed and another panel shifted into
                // its position. Activate the new panel at the same index.
                if let Some(new_active) = self.panels.get(self.active_ix) {
                    new_active.set_active(true, cx);
                }
                self.focus_active_panel(window, cx);
            }
        }
    }

    /// Check to remove self from the parent StackPanel, if there is no panel left
    fn remove_self_if_empty(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panels.is_empty() || self.is_last_panel(cx) {
            return;
        }

        let tab_view = cx.entity().clone();

        if let Some(stack_panel) = self.stack_panel.as_ref() {
            _ = stack_panel.update(cx, |view, cx| {
                view.remove_panel(Arc::new(tab_view), window, cx);
            });
        }
    }

    pub(super) fn set_collapsed(
        &mut self,
        collapsed: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collapsed = collapsed;
        cx.notify();
    }

    fn is_locked(&self, cx: &App) -> bool {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return true;
        };

        if dock_area.read(cx).is_locked() {
            return true;
        }

        if self.zoomed {
            return true;
        }

        self.stack_panel.is_none()
    }

    /// Return true if self or parent only have last panel.
    fn is_last_panel(&self, cx: &App) -> bool {
        if let Some(parent) = &self.stack_panel
            && let Some(stack_panel) = parent.upgrade()
            && !stack_panel.read(cx).is_last_panel(cx)
        {
            return false;
        }

        self.panels.len() <= 1
    }

    /// Return all visible panels
    fn visible_panels<'a>(&'a self, cx: &'a App) -> impl Iterator<Item = Arc<dyn PanelView>> + 'a {
        self.panels.iter().filter_map(|panel| {
            if panel.visible(cx) {
                Some(panel.clone())
            } else {
                None
            }
        })
    }

    /// Return all panel ids
    pub fn panel_ids<'a>(&'a self, cx: &'a App) -> Vec<SharedString> {
        self.panels.iter().map(|panel| panel.panel_id(cx)).collect()
    }

    /// Return true if the tab panel is draggable.
    ///
    /// E.g. if the parent and self only have one panel, it is not draggable.
    fn draggable(&self, cx: &App) -> bool {
        !self.is_locked(cx) && !self.is_last_panel(cx)
    }

    /// Return true if the tab panel is droppable.
    ///
    /// E.g. if the tab panel is locked, it is not droppable.
    fn droppable(&self, cx: &App) -> bool {
        !self.is_locked(cx)
    }

    fn render_toolbar(
        &self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity().clone();
        let build_popup_menu = move |this, cx: &App| view.read(cx).popup_menu(this, cx);
        let toolbar = self.toolbar_buttons(window, cx);

        h_flex()
            .p_0p5()
            .gap_1p5()
            .occlude()
            .rounded_full()
            .children(toolbar.into_iter().map(|btn| btn.small().ghost()))
            .child(
                Button::new("menu")
                    .icon(IconName::Ellipsis)
                    .small()
                    .ghost()
                    .dropdown_menu({
                        let closable = state.closable;

                        move |this, _window, cx| {
                            build_popup_menu(this, cx)
                                .when(closable, |this| {
                                    this.separator().menu("Close Tab", Box::new(ClosePanel))
                                })
                                .menu("Close All Tabs", Box::new(CloseAllPanels))
                                .menu("Reopen Closed Tab", Box::new(ReopenClosedPanel))
                        }
                    })
                    .anchor(Anchor::TopRight),
            )
    }

    fn render_dock_toggle_button(
        &self,
        placement: DockPlacement,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Button> {
        let dock_area = self.dock_area.upgrade()?.read(cx);

        if self.zoomed {
            return None;
        }

        if !dock_area.toggle_button_visible {
            return None;
        }

        if !dock_area.is_dock_collapsible(placement, cx) {
            return None;
        }

        let view_entity_id = cx.entity().entity_id();
        let toggle_button_panels = dock_area.toggle_button_panels;

        // Check if current TabPanel's entity_id matches the one stored in DockArea for this placement
        if !match placement {
            DockPlacement::Left => {
                dock_area.left_dock.is_some() && toggle_button_panels.left == Some(view_entity_id)
            }
            DockPlacement::Right => {
                dock_area.right_dock.is_some() && toggle_button_panels.right == Some(view_entity_id)
            }
            DockPlacement::Bottom => {
                dock_area.bottom_dock.is_some()
                    && toggle_button_panels.bottom == Some(view_entity_id)
            }
            DockPlacement::Center => unreachable!(),
        } {
            return None;
        }

        let is_open = dock_area.is_dock_open(placement, cx);

        let icon = match placement {
            DockPlacement::Left => {
                if is_open {
                    IconName::PanelLeft
                } else {
                    IconName::PanelLeftOpen
                }
            }
            DockPlacement::Right => {
                if is_open {
                    IconName::PanelRight
                } else {
                    IconName::PanelRightOpen
                }
            }
            DockPlacement::Bottom => {
                if is_open {
                    IconName::PanelBottom
                } else {
                    IconName::PanelBottomOpen
                }
            }
            DockPlacement::Center => unreachable!(),
        };

        Some(
            Button::new(SharedString::from(format!("toggle-dock:{:?}", placement)))
                .icon(icon)
                .small()
                .ghost()
                .tab_stop(false)
                .tooltip(match is_open {
                    true => "Collapse",
                    false => "Expand",
                })
                .on_click(cx.listener({
                    let dock_area = self.dock_area.clone();
                    move |_this, _ev, window, cx| {
                        _ = dock_area.update(cx, |dock_area, cx| {
                            dock_area.toggle_dock(placement, window, cx);
                        });
                    }
                })),
        )
    }

    fn render_title_bar(
        &self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Get the dock area entity
        let Some(dock_area) = self.dock_area.upgrade() else {
            // Return a default element if the dock area is not available
            return div().into_any_element();
        };

        let left_dock_button = self.render_dock_toggle_button(DockPlacement::Left, window, cx);
        let bottom_dock_button = self.render_dock_toggle_button(DockPlacement::Bottom, window, cx);
        let right_dock_button = self.render_dock_toggle_button(DockPlacement::Right, window, cx);

        let has_extend_dock_button = left_dock_button.is_some() || bottom_dock_button.is_some();
        let tabs_count = self.panels.len();
        let is_bottom_dock = bottom_dock_button.is_some();

        if tabs_count == 1 && dock_area.read(cx).panel_style == PanelStyle::Default {
            let panel = self.panels.first().unwrap();

            if !panel.visible(cx) {
                return div().into_any_element();
            }

            return h_flex()
                .justify_between()
                .items_center()
                .line_height(rems(1.0))
                .h(TABBAR_HEIGHT)
                .py_2()
                .pl_3()
                .pr_2()
                .rounded_t(cx.theme().radius_lg)
                .bg(cx.theme().panel_background)
                .when(left_dock_button.is_some(), |this| this.pl_2())
                .when(right_dock_button.is_some(), |this| this.pr_2())
                .when(has_extend_dock_button, |this| {
                    this.child(
                        h_flex()
                            .flex_shrink_0()
                            .mr_1()
                            .gap_1()
                            .children(left_dock_button)
                            .children(bottom_dock_button),
                    )
                })
                .child(
                    div()
                        .id("tab")
                        .flex_1()
                        .px_2()
                        .min_w_16()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(
                            div()
                                .w_full()
                                .text_ellipsis()
                                .text_sm()
                                .child(panel.title(cx)),
                        )
                        .when(state.draggable, |this| {
                            this.on_drag(
                                DragPanel {
                                    panel: panel.clone(),
                                    tab_panel: cx.entity(),
                                },
                                |drag, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.new(|_| drag.clone())
                                },
                            )
                        }),
                )
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .ml_1()
                        .gap_1()
                        .child(self.render_toolbar(state, window, cx))
                        .children(right_dock_button),
                )
                .into_any_element();
        }

        TabBar::new("tab-bar")
            .track_scroll(&self.tab_bar_scroll_handle)
            .h(TABBAR_HEIGHT)
            .bg(cx.theme().panel_background)
            .rounded_t(cx.theme().radius_lg)
            .when(has_extend_dock_button, |this| {
                this.prefix(
                    h_flex()
                        .items_center()
                        .top_0()
                        .right(-px(1.))
                        .pl_0p5()
                        .pr_1()
                        .children(left_dock_button)
                        .children(bottom_dock_button),
                )
            })
            .children(self.panels.iter().enumerate().filter_map(|(ix, panel)| {
                let disabled = self.collapsed;
                let mut active = state.active_panel.as_ref() == Some(panel);

                // If the panel is not visible, hide the tabbar
                if !panel.visible(cx) {
                    return None;
                }

                // Always not show active tab style, if the panel is collapsed
                if self.collapsed {
                    active = false;
                }

                Some(
                    Tab::new()
                        .ix(ix)
                        .tab_bar_prefix(has_extend_dock_button)
                        .child(panel.title(cx))
                        .selected(active)
                        .disabled(disabled)
                        .suffix(
                            Button::new("close-{ix}")
                                .icon(IconName::Close)
                                .tooltip("Close panel")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener({
                                    let panel = panel.clone();
                                    move |view, _ev, window, cx| {
                                        cx.stop_propagation();
                                        view.remove_panel(&panel, window, cx);
                                    }
                                })),
                        )
                        .on_click(cx.listener({
                            let is_collapsed = self.collapsed;
                            let dock_area = self.dock_area.clone();
                            move |view, _, window, cx| {
                                view.set_active_ix(ix, window, cx);

                                // Open dock if clicked on the collapsed bottom dock
                                if is_bottom_dock && is_collapsed {
                                    _ = dock_area.update(cx, |dock_area, cx| {
                                        dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                                    });
                                }
                            }
                        }))
                        .when(!disabled, |this| {
                            this.on_mouse_down(
                                MouseButton::Middle,
                                cx.listener({
                                    let panel = panel.clone();
                                    move |view, _ev, window, cx| {
                                        view.remove_panel(&panel, window, cx);
                                    }
                                }),
                            )
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.set_active_ix(ix, window, cx);
                            }))
                            .when(state.draggable, |this| {
                                this.on_drag(
                                    DragPanel::new(panel.clone(), cx.entity().clone()),
                                    |drag, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.new(|_| drag.clone())
                                    },
                                )
                            })
                            .when(state.droppable, |this| {
                                this.drag_over::<DragPanel>(|this, _, _, cx| {
                                    this.rounded_l_none()
                                        .border_l_2()
                                        .border_r_0()
                                        .border_color(cx.theme().border)
                                })
                                .on_drop(cx.listener(
                                    move |this, drag: &DragPanel, window, cx| {
                                        this.will_split_placement = None;
                                        this.on_drop(drag, Some(ix), true, window, cx)
                                    },
                                ))
                            })
                        }),
                )
            }))
            .last_empty_space(
                // empty space to allow move to last tab right
                div()
                    .id("tab-bar-empty-space")
                    .h_full()
                    .flex_grow_1()
                    .min_w_16()
                    .when(state.droppable, |this| {
                        let view = cx.entity();

                        this.drag_over::<DragPanel>(|this, _d, _window, cx| {
                            this.bg(cx.theme().surface_background)
                        })
                        .on_drop(cx.listener(
                            move |this, drag: &DragPanel, window, cx| {
                                this.will_split_placement = None;

                                let ix = if drag.tab_panel == view {
                                    Some(tabs_count - 1)
                                } else {
                                    None
                                };

                                this.on_drop(drag, ix, false, window, cx)
                            },
                        ))
                    }),
            )
            .when(!self.collapsed, |this| {
                this.suffix(
                    h_flex()
                        .items_center()
                        .top_0()
                        .right_0()
                        .h_full()
                        .px_0p5()
                        .gap_1()
                        .child(self.render_toolbar(state, window, cx))
                        .when_some(right_dock_button, |this, btn| this.child(btn)),
                )
            })
            .into_any_element()
    }

    fn render_active_panel(
        &self,
        state: &TabState,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.collapsed {
            return Empty {}.into_any_element();
        }

        let Some(active_panel) = state.active_panel.as_ref() else {
            return Empty {}.into_any_element();
        };

        v_flex()
            .id("tab-content")
            .group("")
            .overflow_hidden()
            .flex_1()
            .child(
                div()
                    .size_full()
                    .rounded_b(cx.theme().radius_lg)
                    .bg(cx.theme().panel_background)
                    .overflow_hidden()
                    .child(
                        active_panel
                            .view()
                            .cached(gpui::StyleRefinement::default().v_flex().size_full()),
                    ),
            )
            .when(state.droppable, |this| {
                this.on_drag_move(cx.listener(Self::on_panel_drag_move))
                    .child(
                        div()
                            .invisible()
                            .absolute()
                            .child(
                                div()
                                    .rounded(cx.theme().radius_lg)
                                    .border_1()
                                    .border_color(cx.theme().element_disabled)
                                    .bg(cx.theme().drop_target_background)
                                    .size_full(),
                            )
                            .map(|this| match self.will_split_placement {
                                Some(placement) => {
                                    let size = DefiniteLength::Fraction(0.35);
                                    match placement {
                                        Placement::Left => this.left_0().top_0().bottom_0().w(size),
                                        Placement::Right => {
                                            this.right_0().top_0().bottom_0().w(size)
                                        }
                                        Placement::Top => this.top_0().left_0().right_0().h(size),
                                        Placement::Bottom => {
                                            this.bottom_0().left_0().right_0().h(size)
                                        }
                                    }
                                }
                                None => this.top_0().left_0().size_full(),
                            })
                            .group_drag_over::<DragPanel>("", |this| this.visible())
                            .on_drop(cx.listener(|this, drag: &DragPanel, window, cx| {
                                this.on_drop(drag, None, true, window, cx)
                            })),
                    )
            })
            .into_any_element()
    }

    /// Calculate the split direction based on the current mouse position
    fn on_panel_drag_move(
        &mut self,
        drag: &DragMoveEvent<DragPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = drag.bounds;
        let position = drag.event.position;

        // Check the mouse position to determine the split direction
        if position.x < bounds.left() + bounds.size.width * 0.35 {
            self.will_split_placement = Some(Placement::Left);
        } else if position.x > bounds.left() + bounds.size.width * 0.65 {
            self.will_split_placement = Some(Placement::Right);
        } else if position.y < bounds.top() + bounds.size.height * 0.35 {
            self.will_split_placement = Some(Placement::Top);
        } else if position.y > bounds.top() + bounds.size.height * 0.65 {
            self.will_split_placement = Some(Placement::Bottom);
        } else {
            // center to merge into the current tab
            self.will_split_placement = None;
        }

        cx.notify();
    }

    /// Handle the drop event when dragging a panel
    ///
    /// - `active` - When true, the panel will be active after the drop
    fn on_drop(
        &mut self,
        drag: &DragPanel,
        ix: Option<usize>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = drag.panel.clone();
        let is_same_tab = drag.tab_panel == cx.entity();

        // If target is same tab, and it is only one panel, do nothing.
        if is_same_tab
            && ix.is_none()
            && (self.will_split_placement.is_none() || self.panels.len() == 1)
        {
            return;
        }

        // Here is looks like remove_panel on a same item, but it difference.
        //
        // We must to split it to remove_panel, unless it will be crash by error:
        // Cannot update ui::dock::tab_panel::TabPanel while it is already being updated
        if is_same_tab {
            self.detach_panel(&panel, window, cx);
        } else {
            drag.tab_panel.update(cx, |view, cx| {
                view.detach_panel(&panel, window, cx);
                view.remove_self_if_empty(window, cx);
            });
        }

        // Insert into new tabs
        if let Some(placement) = self.will_split_placement {
            self.split_panel(panel, placement, None, window, cx);
        } else if let Some(ix) = ix {
            self.insert_panel_at(panel, ix, window, cx)
        } else {
            self.add_panel_with_active(panel, active, window, cx)
        }

        self.remove_self_if_empty(window, cx);
        cx.emit(PanelEvent::LayoutChanged);
    }

    /// Add panel with split placement
    fn split_panel(
        &self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock_area = self.dock_area.clone();
        // wrap the panel in a TabPanel
        let new_tab_panel = cx.new(|cx| Self::new(None, dock_area.clone(), window, cx));
        new_tab_panel.update(cx, |view, cx| {
            view.add_panel(panel, window, cx);
        });

        let stack_panel = match self.stack_panel.as_ref().and_then(|panel| panel.upgrade()) {
            Some(panel) => panel,
            None => return,
        };

        let parent_axis = stack_panel.read(cx).axis;

        let ix = stack_panel
            .read(cx)
            .index_of_panel(Arc::new(cx.entity().clone()))
            .unwrap_or_default();

        if parent_axis.is_vertical() && placement.is_vertical() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else if parent_axis.is_horizontal() && placement.is_horizontal() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else {
            // 1. Create new StackPanel with new axis
            // 2. Move cx.entity() from parent StackPanel to the new StackPanel
            // 3. Add the new TabPanel to the new StackPanel at the correct index
            // 4. Add new StackPanel to the parent StackPanel at the correct index
            let tab_panel = cx.entity().clone();

            // Try to use the old stack panel, not just create a new one, to avoid too many nested stack panels
            let new_stack_panel = if stack_panel.read(cx).panels_len() <= 1 {
                stack_panel.update(cx, |view, cx| {
                    view.remove_all_panels(window, cx);
                    view.set_axis(placement.axis(), window, cx);
                });
                stack_panel.clone()
            } else {
                cx.new(|cx| {
                    let mut panel = StackPanel::new(placement.axis(), window, cx);
                    panel.parent = Some(stack_panel.downgrade());
                    panel
                })
            };

            new_stack_panel.update(cx, |view, cx| match placement {
                Placement::Left | Placement::Top => {
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                }
                Placement::Right | Placement::Bottom => {
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                }
            });

            if stack_panel != new_stack_panel {
                stack_panel.update(cx, |view, cx| {
                    view.replace_panel(
                        Arc::new(tab_panel.clone()),
                        new_stack_panel.clone(),
                        window,
                        cx,
                    );
                });
            }

            cx.spawn_in(window, async move |_, cx| {
                cx.update(|window, cx| {
                    tab_panel.update(cx, |view, cx| view.remove_self_if_empty(window, cx))
                })
            })
            .detach()
        }

        cx.emit(PanelEvent::LayoutChanged);
    }

    fn focus_active_panel(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(active_panel) = self.active_panel(cx) {
            window.focus(&active_panel.focus_handle(cx), cx);
        }
    }

    pub fn cycle(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let visible: Vec<_> = self
            .panels
            .iter()
            .enumerate()
            .filter(|(_, p)| p.visible(cx))
            .map(|(i, _)| i)
            .collect();
        if visible.is_empty() {
            return;
        }
        let current = visible
            .iter()
            .position(|i| *i == self.active_ix)
            .unwrap_or(0);
        let next = if backwards {
            (current + visible.len() - 1) % visible.len()
        } else {
            (current + 1) % visible.len()
        };
        self.set_active_ix(visible[next], window, cx);
    }

    fn on_action_close_panel(
        &mut self,
        _ev: &ClosePanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = self.active_panel(cx) {
            self.remove_panel(&panel, window, cx);
        }
    }
}

impl Focusable for TabPanel {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        if let Some(active_panel) = self.active_panel(cx) {
            active_panel.focus_handle(cx)
        } else {
            self.focus_handle.clone()
        }
    }
}

impl EventEmitter<DismissEvent> for TabPanel {}

impl EventEmitter<PanelEvent> for TabPanel {}

impl Render for TabPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let focus_handle = self.focus_handle(cx);
        let active_panel = self.active_panel(cx);

        let state = TabState {
            closable: self.closable(cx),
            draggable: self.draggable(cx),
            droppable: self.droppable(cx),
            active_panel,
        };

        div()
            .when(!self.collapsed, |this| {
                this.on_action(cx.listener(Self::on_action_close_panel))
                    .on_action(
                        cx.listener(|this, _: &NextPanel, window, cx| {
                            this.cycle(false, window, cx)
                        }),
                    )
                    .on_action(cx.listener(|this, _: &PreviousPanel, window, cx| {
                        this.cycle(true, window, cx)
                    }))
            })
            .id("tab-panel")
            .track_focus(&focus_handle)
            .size_full()
            .p_1()
            .overflow_hidden()
            .child(
                v_flex()
                    .rounded(cx.theme().radius_lg)
                    .when(cx.theme().shadow, |this| this.shadow_xs())
                    .size_full()
                    .tab_group()
                    .child(self.render_title_bar(&state, window, cx))
                    .child(self.render_active_panel(&state, window, cx)),
            )
    }
}
