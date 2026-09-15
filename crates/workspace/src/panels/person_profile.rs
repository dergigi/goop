use gpui::{AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle,
    Focusable, IntoElement, ParentElement, Render, SharedString, Styled, Window};
use nostr_sdk::prelude::PublicKey;
use ui::dock::{Panel, PanelEvent};
use ui::scroll::ScrollableElement;
use ui::v_flex;
use crate::dialogs::screening::{self, Screening};

pub fn init(key: PublicKey, window: &mut Window, cx: &mut App) -> Entity<PersonProfile> {
    let details = screening::init(key, window, cx);
    cx.new(|cx| PersonProfile { key, details, focus_handle: cx.focus_handle() })
}

pub struct PersonProfile {
    key: PublicKey,
    details: Entity<Screening>,
    focus_handle: FocusHandle,
}

impl Panel for PersonProfile {
    fn panel_id(&self) -> SharedString { format!("profile-{}", self.key.to_hex()).into() }
    fn title(&self, _: &App) -> AnyElement { "Profile".into_any_element() }
}
impl EventEmitter<PanelEvent> for PersonProfile {}
impl Focusable for PersonProfile {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus_handle.clone() }
}
impl Render for PersonProfile {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex().size_full().p_4().overflow_y_scrollbar().child(self.details.clone())
    }
}
