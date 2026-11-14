use watchsync::{engine, mpv, net};

use engine::{FileStatus, Phase, Snapshot, UiCmd};
use gpui::{
    App, Application, Bounds, ClipboardItem, Context, FontWeight, PathPromptOptions,
    SharedString, Task, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
use watchsync::sync::format_time;

// ------------------------------------------------------------------ palette

const BG: u32 = 0x14151a;
const PANEL: u32 = 0x1d1f26;
const BORDER: u32 = 0x2c2f3a;
const TEXT: u32 = 0xe8e8ee;
const MUTED: u32 = 0x8b8fa3;
const ACCENT: u32 = 0xe0a43a;
const GREEN: u32 = 0x5cc28a;
const RED: u32 = 0xe5675f;

struct WatchSync {
    snap: Snapshot,
    cmd: async_channel::Sender<UiCmd>,
    /// Local, UI-only messages ("Copied!", "Clipboard is empty").
    notice: Option<SharedString>,
    setup: Setup,
    _updates: Task<()>,
}

/// Whether the video player (mpv) is ready.
#[derive(Clone, PartialEq)]
enum Setup {
    Ready,
    Missing,
    Installing,
    Failed(String),
}

impl Setup {
    fn check() -> Self {
        if mpv::is_installed() { Setup::Ready } else { Setup::Missing }
    }
}

/// Installs Homebrew and mpv, for pasting into Terminal.
const INSTALL_COMMAND: &str = r#"/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)" && eval "$(/opt/homebrew/bin/brew shellenv 2>/dev/null || /usr/local/bin/brew shellenv)" && brew install mpv"#;

impl WatchSync {
    fn install_mpv(&mut self, cx: &mut Context<Self>) {
        self.setup = Setup::Installing;
        cx.notify();
        let job = cx.background_executor().spawn(async { mpv::install_with_brew() });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |view: &mut WatchSync, cx| {
                view.setup = match result {
                    Ok(()) => Setup::Ready,
                    Err(e) => Setup::Failed(e.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn copy_install_command(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(INSTALL_COMMAND.to_string()));
        self.notice = Some(
            "Copied. Open Terminal, paste, press Return, then come back and click Check again."
                .into(),
        );
        cx.notify();
    }

    /// Shown until mpv is installed.
    fn render_setup(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let has_brew = mpv::find_brew().is_some();
        let mut p = panel()
            .child(heading("One-time setup"))
            .child(div().child(
                "WatchSync plays movies with mpv, a free video player. It isn't installed yet.",
            ));
        p = match (&self.setup, has_brew) {
            (Setup::Installing, _) => p.child(
                div()
                    .text_color(rgb(ACCENT))
                    .child("Installing mpv with Homebrew… this takes a minute or two."),
            ),
            (_, true) => p.child(
                div()
                    .flex()
                    .gap_2()
                    .child(button("install", "Install mpv", true).on_click(
                        cx.listener(|this, _, _, cx| this.install_mpv(cx)),
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child("Runs: brew install mpv"),
                    ),
            ),
            (_, false) => p
                .child(div().text_sm().text_color(rgb(MUTED)).child(
                    "Homebrew (the Mac package manager) isn't installed either. Copy this \
                     command, paste it into the Terminal app and press Return. It installs \
                     Homebrew and then mpv (it asks for your Mac password).",
                ))
                .child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(rgb(BG))
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(INSTALL_COMMAND),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(button("copy-install", "Copy command", true).on_click(
                            cx.listener(|this, _, _, cx| this.copy_install_command(cx)),
                        ))
                        .child(button("recheck", "Check again", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.setup = Setup::check();
                                if this.setup == Setup::Ready {
                                    this.notice = None;
                                }
                                cx.notify();
                            }),
                        )),
                ),
        };
        if let Setup::Failed(e) = &self.setup {
            p = p.child(div().text_sm().text_color(rgb(RED)).child(e.clone()));
        }
        p
    }

    fn send(&self, cmd: UiCmd) {
        let _ = self.cmd.try_send(cmd);
    }

    fn join_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let code = cx.read_from_clipboard().and_then(|item| item.text());
        match code {
            Some(code) if net::decode_code(&code).is_ok() => {
                self.notice = None;
                self.send(UiCmd::Join(code));
            }
            _ => {
                self.notice = Some(
                    "No room code on the clipboard. Copy the code your friend sent you, then click Join."
                        .into(),
                );
            }
        }
        cx.notify();
    }

    fn copy_code(&mut self, cx: &mut Context<Self>) {
        if let Some(code) = &self.snap.code {
            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
            self.notice = Some("Room code copied. Send it to your friends.".into());
            cx.notify();
        }
    }

    fn choose_movie(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Watch this".into()),
        });
        let cmd = self.cmd.clone();
        cx.spawn(async move |_, _| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = cmd.send(UiCmd::OpenFile(path)).await;
            }
        })
        .detach();
    }
}

// ------------------------------------------------------------------ widgets

fn button(id: &'static str, label: impl Into<SharedString>, primary: bool) -> gpui::Stateful<gpui::Div> {
    let label: SharedString = label.into();
    let (bg, fg) = if primary {
        (rgb(ACCENT), rgb(0x1a1206))
    } else {
        (rgb(PANEL), rgb(TEXT))
    };
    div()
        .id(id)
        .px_4()
        .py_2()
        .rounded_md()
        .bg(bg)
        .text_color(fg)
        .border_1()
        .border_color(if primary { rgb(ACCENT) } else { rgb(BORDER) })
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .active(|s| s.opacity(0.75))
        .child(label)
}

fn panel() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_lg()
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgb(BORDER))
}

fn heading(text: &'static str) -> gpui::Div {
    div()
        .text_xs()
        .text_color(rgb(MUTED))
        .child(text.to_uppercase())
}

// ------------------------------------------------------------------ views

impl WatchSync {
    fn render_home(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("Watch the same movie with friends, in sync. No accounts."),
            )
            .child(
                panel()
                    .child(heading("Start"))
                    .child(div().child(format!("You'll appear as {}", self.snap.name)))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(button("host", "Host a room", true).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.notice = None;
                                    this.send(UiCmd::Host);
                                    cx.notify();
                                }),
                            ))
                            .child(button("join", "Join with copied code", false).on_click(
                                cx.listener(|this, _, _, cx| this.join_from_clipboard(cx)),
                            )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child("To join: copy the room code your friend sent you, then click Join."),
                    ),
            )
    }

    fn render_room(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let s = &self.snap;

        let room = panel()
            .child(heading(if s.is_host { "Your room" } else { "Room" }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().child(if !s.connected {
                        "Disconnected"
                    } else if s.is_host {
                        "You're hosting. Share the code so friends can join."
                    } else {
                        "Connected to the host."
                    }))
                    .when(s.is_host, |d| {
                        d.child(button("copy", "Copy room code", false).on_click(
                            cx.listener(|this, _, _, cx| this.copy_code(cx)),
                        ))
                    }),
            );

        let movie = panel()
            .child(heading("Movie"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .truncate()
                            .text_color(if s.file_name.is_some() { rgb(TEXT) } else { rgb(MUTED) })
                            .child(
                                s.file_name
                                    .clone()
                                    .unwrap_or_else(|| "Choose your copy of the movie".into()),
                            ),
                    )
                    .child(
                        button(
                            "choose",
                            if s.player_open { "Change…" } else { "Choose movie…" },
                            !s.player_open,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.choose_movie(cx))),
                    ),
            )
            .when(s.file_name.is_some() && !s.player_open, |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child("The player window was closed. Choose the movie again to reopen it."),
                )
            });

        let progress = if s.duration > 0.0 {
            (s.position / s.duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let controls = panel()
            .child(heading("Playback (synced for everyone)"))
            .child(
                div()
                    .h(px(6.))
                    .w_full()
                    .rounded_full()
                    .bg(rgb(BORDER))
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(rgb(ACCENT))
                            .w(gpui::relative(progress)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(button("back", "−10s", false).on_click(
                        cx.listener(|this, _, _, _| this.send(UiCmd::SeekBy(-10.0))),
                    ))
                    .child(
                        button("toggle", if s.playing { "Pause" } else { "Play" }, true)
                            .on_click(cx.listener(|this, _, _, _| this.send(UiCmd::TogglePlay))),
                    )
                    .child(button("fwd", "+10s", false).on_click(
                        cx.listener(|this, _, _, _| this.send(UiCmd::SeekBy(10.0))),
                    ))
                    .child(div().flex_1())
                    .child(div().text_color(rgb(MUTED)).child(format!(
                        "{} / {}",
                        format_time(s.position),
                        if s.duration > 0.0 { format_time(s.duration) } else { "--:--".into() }
                    ))),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(s.last_action.clone().unwrap_or_else(|| {
                        "Space in the movie window also pauses for everyone.".into()
                    })),
            );

        let mut people = panel().child(heading("Watching"));
        if s.peers.is_empty() {
            people = people.child(div().text_color(rgb(MUTED)).child("Just you so far."));
        }
        for p in &s.peers {
            let (file_text, file_color) = match p.file {
                FileStatus::Match => ("same file", GREEN),
                FileStatus::Mismatch => ("different file!", RED),
                FileStatus::NoFile => ("no movie chosen", MUTED),
                FileStatus::Unknown => ("file chosen", MUTED),
            };
            let drift = p.drift_ms.map(|d| {
                let color = if d.abs() < 80.0 { GREEN } else if d.abs() < 500.0 { ACCENT } else { RED };
                (format!("{:+.0} ms", d), color)
            });
            people = people.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().size(px(8.)).rounded_full().bg(rgb(GREEN)))
                    .child(div().font_weight(FontWeight::MEDIUM).child(p.name.clone()))
                    .when(p.is_host, |d| {
                        d.child(div().text_xs().text_color(rgb(ACCENT)).child("HOST"))
                    })
                    .child(div().flex_1())
                    .child(div().text_sm().text_color(rgb(file_color)).child(file_text))
                    .when_some(drift, |d, (text, c)| {
                        d.child(div().w(px(70.)).text_sm().text_color(rgb(c)).child(text))
                    }),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(room)
            .child(movie)
            .child(controls)
            .child(people)
    }
}

impl Render for WatchSync {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.snap.phase {
            _ if self.setup != Setup::Ready => self.render_setup(cx),
            Phase::Home => self.render_home(cx),
            Phase::Connecting => div()
                .text_color(rgb(MUTED))
                .child("Connecting… (this can take a few seconds)"),
            Phase::Room => self.render_room(cx),
        };
        let message = self
            .snap
            .error
            .clone()
            .map(|e| (SharedString::from(e), RED))
            .or_else(|| self.notice.clone().map(|n| (n, ACCENT)));

        div()
            .id("root")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("WatchSync"),
            )
            .when_some(message, |d, (text, c)| {
                d.child(
                    div()
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(c))
                        .text_color(rgb(c))
                        .text_sm()
                        .child(text),
                )
            })
            .child(body)
    }
}

/// The Mac's full user name ("Akshat Dubey"), falling back to $USER.
fn display_name() -> String {
    std::process::Command::new("id")
        .arg("-F")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "Friend".into())
}

fn main() {
    let name = display_name();
    let (snap_tx, snap_rx) = async_channel::bounded::<Snapshot>(1);
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<UiCmd>();

    // Networking and player control live on a tokio runtime in its own thread.
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        runtime.block_on(engine::run(name, snap_tx, cmd_rx));
    });

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.), px(680.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let updates = cx.spawn(async move |this, cx| {
                        while let Ok(snap) = snap_rx.recv().await {
                            let alive = this.update(cx, |view: &mut WatchSync, cx| {
                                if view.snap.phase != snap.phase {
                                    view.notice = None;
                                }
                                view.snap = snap;
                                cx.notify();
                            });
                            if alive.is_err() {
                                break;
                            }
                        }
                    });
                    WatchSync {
                        snap: Snapshot::default(),
                        cmd: cmd_tx,
                        notice: None,
                        setup: Setup::check(),
                        _updates: updates,
                    }
                })
            },
        )
        .unwrap();
        cx.on_window_closed(|cx| {
            mpv::kill_player();
            cx.quit();
        })
        .detach();
        cx.activate(true);
    });
}
