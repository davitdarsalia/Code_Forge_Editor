use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    PathPromptOptions, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use crate::git;
use crate::theme::{Palette, ca};
use crate::ui::{icon, text_button};

pub enum CloneEvent {
    Dismissed,
    Started(String),
    Failed(String),
    Cloned(PathBuf),
}

pub struct CloneDialog {
    input: Entity<InputState>,
    open: bool,
    busy: bool,
    _subscription: Subscription,
}

impl EventEmitter<CloneEvent> for CloneDialog {}

impl CloneDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("https://github.com/user/repo.git"));
        let subscription = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.start(window, cx);
            }
        });
        CloneDialog { input, open: false, busy: false, _subscription: subscription }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.open = false;
        cx.emit(CloneEvent::Dismissed);
        cx.notify();
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.input.read(cx).value().trim().to_string();
        if url.is_empty() || self.busy {
            return;
        }
        if git::repo_name(&url).is_none() {
            cx.emit(CloneEvent::Failed("That does not look like a repository URL".into()));
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(parent) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                this.busy = true;
                cx.emit(CloneEvent::Started(url.clone()));
                cx.notify();
            });
            let result = cx.background_spawn(async move { git::clone(&url, &parent) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(path) => {
                        this.open = false;
                        this.input.update(cx, |input, cx| input.set_value("", window, cx));
                        cx.emit(CloneEvent::Cloned(path));
                        cx.notify();
                    }
                    Err(err) => {
                        cx.emit(CloneEvent::Failed(format!("Clone failed: {err}")));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

impl Render for CloneDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("clone-overlay")
            .absolute()
            .inset_0()
            .bg(ca(0x000000, 0.45))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(120.))
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                this.close(cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .id("clone-dialog")
                    .w(px(480.))
                    .p(px(18.))
                    .rounded(px(12.))
                    .bg(Palette::toolbar_bg())
                    .border_1()
                    .border_color(Palette::hairline())
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(icon(IconName::FolderGit2, 18., Palette::accent()))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(Palette::text())
                                    .child("Clone Git Repository"),
                            ),
                    )
                    .child(Input::new(&self.input))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(Palette::faint())
                            .child("You will pick the parent folder next. Uses your git credentials."),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(text_button("clone-cancel", "Cancel", false, cx.listener(|this, _, _, cx| {
                                this.close(cx)
                            })))
                            .child(text_button(
                                "clone-go",
                                if self.busy { "Cloning…" } else { "Clone…" },
                                true,
                                cx.listener(|this, _, window, cx| this.start(window, cx)),
                            )),
                    ),
            )
    }
}
