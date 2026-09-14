use std::any::TypeId;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use instant::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, Animation, AnimationExt, AnyElement, App, AppContext, ClickEvent, ClipboardItem, Context,
    DismissEvent, ElementId, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement, StyleRefinement, Styled,
    Subscription, Window, div, px, relative,
};
use theme::ActiveTheme;

use crate::animation::cubic_bezier;
use crate::button::{Button, ButtonVariants as _};
use crate::{Icon, IconName, Sizable as _, Size, StyledExt, h_flex, v_flex};

#[derive(Debug, Clone, Copy, Default)]
pub enum NotificationKind {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl NotificationKind {
    fn icon(&self, cx: &App) -> Icon {
        match self {
            Self::Info => Icon::new(IconName::Info)
                .with_size(Size::Medium)
                .text_color(cx.theme().icon),
            Self::Success => Icon::new(IconName::CheckCircle)
                .with_size(Size::Medium)
                .text_color(cx.theme().icon_accent),
            Self::Warning => Icon::new(IconName::Warning)
                .with_size(Size::Medium)
                .text_color(cx.theme().text_warning),
            Self::Error => Icon::new(IconName::CloseCircle)
                .with_size(Size::Medium)
                .text_color(cx.theme().danger_foreground),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Hash, Eq)]
pub(crate) enum NotificationId {
    Id(TypeId),
    IdAndElementId(TypeId, ElementId),
}

impl From<TypeId> for NotificationId {
    fn from(type_id: TypeId) -> Self {
        Self::Id(type_id)
    }
}

impl From<(TypeId, ElementId)> for NotificationId {
    fn from((type_id, id): (TypeId, ElementId)) -> Self {
        Self::IdAndElementId(type_id, id)
    }
}

#[allow(clippy::type_complexity)]
/// A notification element.
pub struct Notification {
    /// The id is used make the notification unique.
    /// Then you push a notification with the same id, the previous notification will be replaced.
    ///
    /// None means the notification will be added to the end of the list.
    id: NotificationId,
    style: StyleRefinement,
    kind: Option<NotificationKind>,
    title: Option<SharedString>,
    message: Option<SharedString>,
    icon: Option<Icon>,
    autohide: bool,
    action_builder: Option<Rc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>) -> Button>>,
    content_builder: Option<Rc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>) -> AnyElement>>,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    closing: bool,
}

impl From<String> for Notification {
    fn from(s: String) -> Self {
        Self::new().message(s)
    }
}

impl From<SharedString> for Notification {
    fn from(s: SharedString) -> Self {
        Self::new().message(s)
    }
}

impl From<&'static str> for Notification {
    fn from(s: &'static str) -> Self {
        Self::new().message(s)
    }
}

impl From<(NotificationKind, &'static str)> for Notification {
    fn from((kind, content): (NotificationKind, &'static str)) -> Self {
        Self::new().message(content).with_kind(kind)
    }
}

impl From<(NotificationKind, SharedString)> for Notification {
    fn from((kind, content): (NotificationKind, SharedString)) -> Self {
        Self::new().message(content).with_kind(kind)
    }
}

struct DefaultIdType;

impl Notification {
    /// Create a new notification.
    ///
    /// The default id is a random UUID.
    pub fn new() -> Self {
        let id: SharedString = uuid::Uuid::new_v4().to_string().into();
        let id = (TypeId::of::<DefaultIdType>(), id.into());

        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            title: None,
            message: None,
            kind: None,
            icon: None,
            autohide: true,
            action_builder: None,
            content_builder: None,
            on_click: None,
            closing: false,
        }
    }

    /// Set the message of the notification, default is None.
    pub fn message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Create an info notification with the given message.
    pub fn info(message: impl Into<SharedString>) -> Self {
        Self::new()
            .message(message)
            .with_kind(NotificationKind::Info)
    }

    /// Create a success notification with the given message.
    pub fn success(message: impl Into<SharedString>) -> Self {
        Self::new()
            .message(message)
            .with_kind(NotificationKind::Success)
    }

    /// Create a warning notification with the given message.
    pub fn warning(message: impl Into<SharedString>) -> Self {
        Self::new()
            .message(message)
            .with_kind(NotificationKind::Warning)
    }

    /// Create an error notification with the given message.
    pub fn error(message: impl Into<SharedString>) -> Self {
        Self::new()
            .message(message)
            .with_kind(NotificationKind::Error)
    }

    /// Set the type for unique identification of the notification.
    ///
    /// ```rs
    /// struct MyNotificationKind;
    /// let notification = Notification::new("Hello").id::<MyNotificationKind>();
    /// ```
    pub fn id<T: Sized + 'static>(mut self) -> Self {
        self.id = TypeId::of::<T>().into();
        self
    }

    /// Set the type and id of the notification, used to uniquely identify the notification.
    pub fn type_id<T: Sized + 'static>(mut self, key: impl Into<ElementId>) -> Self {
        self.id = (TypeId::of::<T>(), key.into()).into();
        self
    }

    /// Set the title of the notification, default is None.
    ///
    /// If title is None, the notification will not have a title.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the icon of the notification.
    ///
    /// If icon is None, the notification will use the default icon of the type.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set the type of the notification, default is NotificationType::Info.
    pub fn with_kind(mut self, kind: NotificationKind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// Set the auto hide of the notification, default is true.
    pub fn autohide(mut self, autohide: bool) -> Self {
        self.autohide = autohide;
        self
    }

    /// Set the click callback of the notification.
    pub fn on_click(
        mut self,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(on_click));
        self
    }

    /// Set the action button of the notification.
    ///
    /// When an action is set, the notification will not autohide.
    pub fn action<F>(mut self, action: F) -> Self
    where
        F: Fn(&mut Self, &mut Window, &mut Context<Self>) -> Button + 'static,
    {
        self.action_builder = Some(Rc::new(action));
        self.autohide = false;
        self
    }

    /// Dismiss the notification.
    pub fn dismiss(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        cx.notify();

        // Dismiss the notification after 0.15s to show the animation.
        cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f32(0.15))
                .await;

            cx.update(|cx| {
                if let Some(view) = view.upgrade() {
                    view.update(cx, |view, cx| {
                        view.closing = false;
                        cx.emit(DismissEvent);
                    });
                }
            })
        })
        .detach();
    }

    /// Set the content of the notification.
    pub fn content(
        mut self,
        content: impl Fn(&mut Self, &mut Window, &mut Context<Self>) -> AnyElement + 'static,
    ) -> Self {
        self.content_builder = Some(Rc::new(content));
        self
    }
}

impl Default for Notification {
    fn default() -> Self {
        Self::new()
    }
}

impl EventEmitter<DismissEvent> for Notification {}

impl FluentBuilder for Notification {}

impl Styled for Notification {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Render for Notification {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self
            .content_builder
            .clone()
            .map(|builder| builder(self, window, cx));

        let action = self.action_builder.clone().map(|builder| {
            builder(self, window, cx)
                .small()
                .primary()
                .px_4()
                .font_medium()
        });

        let icon = match self.kind {
            None => self.icon.clone(),
            Some(kind) => Some(kind.icon(cx)),
        };

        let background = match self.kind {
            Some(NotificationKind::Error) => cx.theme().danger_background,
            _ => cx.theme().surface_background,
        };

        let text_color = match self.kind {
            Some(NotificationKind::Error) => cx.theme().danger_foreground,
            _ => cx.theme().text,
        };

        let closing = self.closing;
        let has_title = self.title.is_some();
        let only_message = !has_title && content.is_none() && action.is_none();
        let placement = cx.theme().notification.placement;

        h_flex()
            .id("notification")
            .group("")
            .occlude()
            .relative()
            .w_112()
            .border_1()
            .border_color(cx.theme().border)
            .bg(background)
            .text_color(text_color)
            .rounded(cx.theme().radius_lg)
            .when(cx.theme().shadow, |this| this.shadow_md())
            .p_2()
            .gap_2()
            .justify_start()
            .items_start()
            .when(only_message, |this| this.items_center())
            .refine_style(&self.style)
            .when_some(icon, |this, icon| {
                this.child(div().flex_shrink_0().size_5().child(icon))
            })
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .overflow_hidden()
                    .when_some(self.title.clone(), |this, title| {
                        this.child(h_flex().h_5().text_sm().font_semibold().child(title))
                    })
                    .when_some(self.message.clone(), |this, message| {
                        this.child(
                            div()
                                .text_sm()
                                .when(has_title, |this| this.text_color(cx.theme().text_muted))
                                .line_height(relative(1.3))
                                .child(message),
                        )
                    })
                    .when_some(content, |this, content| this.child(content))
                    .when_some(action, |this, action| {
                        this.gap_2().child(
                            h_flex()
                                .mt_2()
                                .w_full()
                                .flex_1()
                                .justify_end()
                                .child(action),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .when(self.title.is_some() || self.message.is_some(), |this| {
                        this.child(
                            Button::new("copy")
                                .icon(IconName::Copy)
                                .tooltip("Copy notification")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    cx.stop_propagation();
                                    let text = [this.title.as_ref(), this.message.as_ref()]
                                        .into_iter()
                                        .flatten()
                                        .map(|text| text.as_ref())
                                        .collect::<Vec<&str>>()
                                        .join("\n\n");
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                })),
                        )
                    })
                    .child(
                        Button::new("close")
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |this, _ev, window, cx| {
                                this.dismiss(window, cx);
                            })),
                    ),
            )
            .when_some(self.on_click.clone(), |this, on_click| {
                this.on_click(cx.listener(move |view, event, window, cx| {
                    view.dismiss(window, cx);
                    on_click(event, window, cx);
                }))
            })
            .on_aux_click(cx.listener(move |view, event: &ClickEvent, window, cx| {
                if event.is_middle_click() {
                    view.dismiss(window, cx);
                }
            }))
            .with_animation(
                ElementId::NamedInteger("slide-down".into(), closing as u64),
                Animation::new(Duration::from_secs_f64(0.25))
                    .with_easing(cubic_bezier(0.4, 0., 0.2, 1.)),
                move |this, delta| {
                    if closing {
                        let opacity = 1. - delta;
                        let that = this
                            .shadow_none()
                            .opacity(opacity)
                            .when(opacity < 0.85, |this| this.shadow_none());
                        match placement {
                            Anchor::TopRight | Anchor::BottomRight => {
                                let x_offset = px(0.) + delta * px(45.);
                                that.left(px(0.) + x_offset)
                            }
                            Anchor::TopLeft | Anchor::BottomLeft => {
                                let x_offset = px(0.) - delta * px(45.);
                                that.left(px(0.) + x_offset)
                            }
                            Anchor::TopCenter => {
                                let y_offset = px(0.) - delta * px(45.);
                                that.top(px(0.) + y_offset)
                            }
                            Anchor::BottomCenter => {
                                let y_offset = px(0.) + delta * px(45.);
                                that.top(px(0.) + y_offset)
                            }
                            _ => that,
                        }
                    } else {
                        let opacity = delta;
                        let y_offset = match placement {
                            Anchor::TopLeft | Anchor::TopRight | Anchor::TopCenter => {
                                px(-45.) + delta * px(45.)
                            }
                            Anchor::BottomLeft | Anchor::BottomRight | Anchor::BottomCenter => {
                                px(45.) - delta * px(45.)
                            }
                            _ => px(0.),
                        };
                        this.top(px(0.) + y_offset)
                            .opacity(opacity)
                            .when(opacity < 0.85, |this| this.shadow_none())
                    }
                },
            )
    }
}

/// A list of notifications.
pub struct NotificationList {
    /// Notifications that will be auto hidden.
    pub(crate) notifications: VecDeque<Entity<Notification>>,

    /// Whether the notification list is expanded.
    expanded: bool,

    /// Subscriptions
    _subscriptions: HashMap<NotificationId, Subscription>,
}

impl NotificationList {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            notifications: VecDeque::new(),
            expanded: false,
            _subscriptions: HashMap::new(),
        }
    }

    pub fn push(
        &mut self,
        notification: impl Into<Notification>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let notification = notification.into();
        let id = notification.id.clone();
        let autohide = notification.autohide;

        // Remove the notification by id, for keep unique.
        self.notifications.retain(|note| note.read(cx).id != id);

        let notification = cx.new(|_| notification);

        self._subscriptions.insert(
            id.clone(),
            cx.subscribe(&notification, move |view, _, _: &DismissEvent, cx| {
                view.notifications.retain(|note| id != note.read(cx).id);
                view._subscriptions.remove(&id);
            }),
        );

        self.notifications.push_back(notification.clone());

        if autohide {
            // Sleep for 5 seconds to autohide the notification
            cx.spawn_in(window, async move |_this, cx| {
                cx.background_executor().timer(Duration::from_secs(5)).await;

                if let Err(err) =
                    notification.update_in(cx, |note, window, cx| note.dismiss(window, cx))
                {
                    log::error!("failed to auto hide notification: {:?}", err);
                }
            })
            .detach();
        }

        cx.notify();
    }

    pub(crate) fn close(
        &mut self,
        id: impl Into<NotificationId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id: NotificationId = id.into();
        if let Some(n) = self.notifications.iter().find(|n| n.read(cx).id == id) {
            n.update(cx, |note, cx| note.dismiss(window, cx))
        }
        cx.notify();
    }

    pub fn clear(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.notifications.clear();
        cx.notify();
    }

    pub fn notifications(&self) -> Vec<Entity<Notification>> {
        self.notifications.iter().cloned().collect()
    }
}

impl Render for NotificationList {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let size = window.viewport_size();
        let items = self.notifications.iter().rev().take(10).rev().cloned();

        let placement = cx.theme().notification.placement;
        let margins = &cx.theme().notification.margins;

        v_flex()
            .id("notification-list")
            .max_h(size.height)
            .pt(margins.top)
            .pb(margins.bottom)
            .gap_3()
            .when(
                matches!(placement, Anchor::TopRight),
                |this| this.pr(margins.right), // ignore left
            )
            .when(
                matches!(placement, Anchor::TopLeft),
                |this| this.pl(margins.left), // ignore right
            )
            .when(
                matches!(placement, Anchor::BottomLeft),
                |this| this.flex_col_reverse().pl(margins.left), // ignore right
            )
            .when(
                matches!(placement, Anchor::BottomRight),
                |this| this.flex_col_reverse().pr(margins.right), // ignore left
            )
            .when(matches!(placement, Anchor::BottomCenter), |this| {
                this.flex_col_reverse()
            })
            .on_hover(cx.listener(|view, hovered, _, cx| {
                view.expanded = *hovered;
                cx.notify()
            }))
            .children(items)
    }
}
