//! Middle-click autoscroll, as on Windows and Linux: press the middle
//! button and the page scrolls toward the pointer, faster the further it
//! strays. Hold and drag to scroll until release; click without moving
//! and it keeps going until the next click.
//!
//! It moves the innermost scrolling pane under the point that was clicked
//! -- the page, a file list, a log -- so every pane that scrolls has to
//! get its handle from [`Hub::scroller`].

use crate::hub::Hub;
use crate::ui::{icon, palette};
use gpui::{
    div, point, px, AnyElement, Context, IntoElement as _, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, ScrollHandle, Styled as _,
    UniformListScrollHandle, Window,
};
use std::time::Duration;

/// How far the pointer can wander before anything moves.
const DEAD_ZONE: f32 = 10.0;
/// Further than this from where it started, a press counts as a drag.
const DRAG: f32 = 6.0;

pub struct AutoScroll {
    origin: Point<Pixels>,
    pointer: Point<Pixels>,
    dragged: bool,
}

/// Pixels per frame along one axis for an offset from the origin.
fn speed(offset: f32) -> f32 {
    let past = offset.abs() - DEAD_ZONE;
    if past <= 0.0 {
        return 0.0;
    }
    offset.signum() * (past / 8.0).powf(1.3)
}

impl Hub {
    pub fn autoscroll_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Middle {
            return;
        }
        // A second middle click ends a click-started scroll.
        if self.autoscroll.take().is_some() {
            cx.notify();
            return;
        }
        self.autoscroll = Some(AutoScroll {
            origin: event.position,
            pointer: event.position,
            dragged: false,
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            match this.update(cx, |hub, cx| hub.autoscroll_tick(cx)) {
                Ok(true) => {}
                _ => break,
            }
        })
        .detach();
    }

    /// One frame's worth of scrolling; false once it has stopped.
    fn autoscroll_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(a) = &self.autoscroll else {
            return false;
        };
        let d = a.pointer - a.origin;
        let (dx, dy) = (speed(f32::from(d.x)), speed(f32::from(d.y)));
        if dx == 0.0 && dy == 0.0 {
            return true;
        }
        // The smallest pane drawn last frame that holds the origin and
        // can scroll the way the pointer is pulling.
        let target = self
            .scrollers
            .values()
            .filter(|(_, frame)| *frame == self.frame)
            .map(|(handle, _)| handle)
            .filter(|h| h.bounds().contains(&a.origin))
            .filter(|h| {
                let max = h.max_offset();
                (dy != 0.0 && max.height > px(0.0)) || (dx != 0.0 && max.width > px(0.0))
            })
            .min_by(|a, b| {
                let area = |h: &ScrollHandle| {
                    f32::from(h.bounds().size.width) * f32::from(h.bounds().size.height)
                };
                area(a).total_cmp(&area(b))
            });
        if let Some(handle) = target {
            // Offsets run negative as the content moves up.
            let max = handle.max_offset();
            let at = handle.offset();
            handle.set_offset(point(
                (at.x - px(dx)).clamp(-max.width, px(0.0)),
                (at.y - px(dy)).clamp(-max.height, px(0.0)),
            ));
            cx.notify();
        }
        true
    }

    /// The scroll handle for the pane `key`; a pane that scrolls tracks it
    /// so middle-click scrolling can find it.
    pub fn scroller(&mut self, key: &str) -> ScrollHandle {
        let frame = self.frame;
        let entry = self
            .scrollers
            .entry(key.to_string())
            .or_insert_with(|| (ScrollHandle::new(), frame));
        entry.1 = frame;
        entry.0.clone()
    }

    /// [`Hub::scroller`] for a `uniform_list`.
    pub fn list_scroller(&mut self, key: &str) -> UniformListScrollHandle {
        let handle = self
            .list_scrollers
            .entry(key.to_string())
            .or_insert_with(UniformListScrollHandle::new)
            .clone();
        let base = handle.0.borrow().base_handle.clone();
        self.scrollers.insert(key.to_string(), (base, self.frame));
        handle
    }

    pub fn autoscroll_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some(a) = &mut self.autoscroll {
            a.pointer = event.position;
            let d = a.pointer - a.origin;
            if f32::from(d.x).hypot(f32::from(d.y)) > DRAG {
                a.dragged = true;
            }
            cx.notify();
        }
    }

    pub fn autoscroll_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if event.button == MouseButton::Middle
            && self.autoscroll.as_ref().is_some_and(|a| a.dragged)
        {
            self.autoscroll = None;
            cx.notify();
        }
    }

    /// Any other click ends it. Answers whether one was running.
    pub fn autoscroll_cancel(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.autoscroll.take().is_some();
        if was {
            cx.notify();
        }
        was
    }

    /// The marker at the point scrolling is measured from.
    pub fn render_autoscroll(&self) -> Option<AnyElement> {
        let a = self.autoscroll.as_ref()?;
        let p = palette();
        let size = 30.0;
        Some(
            div()
                .absolute()
                .left(a.origin.x - px(size / 2.0))
                .top(a.origin.y - px(size / 2.0))
                .size(px(size))
                .rounded_full()
                .bg(gpui::rgb(p.panel_bg))
                .border_1()
                .border_color(gpui::rgb(p.edge))
                .shadow_md()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(icon("chevron-up", 10.0, p.text_dim))
                .child(div().size(px(3.0)).rounded_full().bg(gpui::rgb(p.text_dim)))
                .child(icon("chevron-down", 10.0, p.text_dim))
                .into_any_element(),
        )
    }
}
