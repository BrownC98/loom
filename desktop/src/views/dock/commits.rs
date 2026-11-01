use std::time::Duration;

use api::canvas::{CommitGraph, GitRef};
use api::ids::RepoId;
use api::repos::Worktree;
use gpui::{
    AppContext, Bounds, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, PathBuilder, Rgba, Stateful, StatefulInteractiveElement, Styled, canvas, div,
    fill, point, px, size, uniform_list,
};

use super::lanes::{LaneState, Row, circle_of, lane_count, lanes};
use super::strokes::{self, LANE_WIDTH, RING, ROW_HEIGHT, drawing};
use super::ui::{self, GAP, PAD, SMALL};
use super::{Dock, Tab};
use crate::canvas::repos::chosen_repo;
use crate::fonts;
use crate::link;
use crate::problem::Failure;
use crate::strings::{Lang, Str};
use crate::theme::Colors;
use crate::views::sidebar::icons::{Glyph, icon};
use crate::views::tooltip::tip;

const PAGE: usize = 100;
const LOAD_AHEAD: usize = 20;
const REFRESH: Duration = Duration::from_secs(3);
const AUTHOR_WIDTH: f32 = 56.0;
const TIME_WIDTH: f32 = 60.0;
const REF_HEIGHT: f32 = 18.0;
const REF_TINT: f32 = 0.22;
const FOLDER: f32 = 11.0;
const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

#[derive(Default)]
pub(super) struct Commits {
    repo: Option<RepoId>,
    epoch: usize,
    poll: usize,
    rows: Vec<Row>,
    end: LaneState,
    widest: usize,
    version: Option<String>,
    worktrees: Vec<Worktree>,
    done: bool,
    busy: bool,
    error: Option<String>,
    now: i64,
}

enum Fetched {
    Same(Vec<Worktree>),
    Reload(CommitGraph, usize),
    Page(CommitGraph),
}

fn lane_color(c: &Colors, index: i32) -> Rgba {
    match index.rem_euclid(5) {
        0 => c.lane_1,
        1 => c.lane_2,
        2 => c.lane_3,
        3 => c.lane_4,
        _ => c.lane_5,
    }
}

fn reload(base: &str, repo: RepoId, rows: usize) -> Result<Fetched, Failure> {
    let limit = PAGE.max(rows);
    link::commits(base, repo, 0, limit).map(|graph| Fetched::Reload(graph, limit))
}

impl Dock {
    pub(super) fn commits_enter(&mut self, cx: &mut Context<Self>) {
        self.commits_follow(cx);
        self.commits.poll += 1;
        let token = self.commits.poll;
        cx.spawn(async move |this, cx| {
            loop {
                let alive = this.update(cx, |dock, cx| {
                    let alive = dock.tab == Tab::Commits && dock.commits.poll == token;
                    if alive {
                        dock.commits_refresh(cx);
                    }
                    alive
                });
                if !alive.unwrap_or(false) {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        })
        .detach();
    }

    pub(super) fn commits_follow(&mut self, cx: &mut Context<Self>) {
        let wanted = {
            let store = self.store.read(cx);
            store
                .snapshot()
                .and_then(|s| chosen_repo(&s.repos, store.settings().repo).map(|r| r.id))
        };
        if wanted == self.commits.repo {
            return;
        }
        self.commits = Commits {
            repo: wanted,
            epoch: self.commits.epoch + 1,
            poll: self.commits.poll,
            ..Commits::default()
        };
        self.commit_bar.handle().set_offset(point(px(0.0), px(0.0)));
        if self.tab == Tab::Commits {
            self.commits_refresh(cx);
        }
        cx.notify();
    }

    fn commits_request(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&str, RepoId) -> Result<Fetched, Failure> + Send + 'static,
    ) {
        let (Some(repo), Some(base)) = (
            self.commits.repo,
            self.store.read(cx).base().map(str::to_owned),
        ) else {
            return;
        };
        if self.commits.busy {
            return;
        }
        self.commits.busy = true;
        let epoch = self.commits.epoch;
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { work(&base, repo) }).await;
            let _ = this.update(cx, |dock, cx| {
                if dock.commits.epoch == epoch {
                    let lang = *cx.global::<Lang>();
                    dock.commits_apply(result.map_err(|failure| failure.text(lang)));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn commits_refresh(&mut self, cx: &mut Context<Self>) {
        let version = self.commits.version.clone();
        let count = self.commits.rows.len();
        self.commits_request(cx, move |base, repo| {
            let Some(version) = version else {
                return reload(base, repo, count);
            };
            let head = link::commits(base, repo, 0, 0)?;
            if head.version == version {
                Ok(Fetched::Same(head.worktrees))
            } else {
                reload(base, repo, count)
            }
        });
    }

    fn commits_more(&mut self, cx: &mut Context<Self>) {
        if self.commits.done || self.commits.version.is_none() {
            return;
        }
        let version = self.commits.version.clone();
        let count = self.commits.rows.len();
        self.commits_request(cx, move |base, repo| {
            let page = link::commits(base, repo, count, PAGE)?;
            if Some(&page.version) == version.as_ref() {
                Ok(Fetched::Page(page))
            } else {
                reload(base, repo, count)
            }
        });
    }

    fn commits_apply(&mut self, result: Result<Fetched, String>) {
        let state = &mut self.commits;
        state.busy = false;
        state.now = chrono::Local::now().timestamp();
        match result {
            Err(error) => state.error = Some(error),
            Ok(Fetched::Same(worktrees)) => {
                state.error = None;
                state.worktrees = worktrees;
            }
            Ok(Fetched::Reload(graph, limit)) => {
                state.error = None;
                state.rows.clear();
                state.end = LaneState::default();
                state.widest = 1;
                state.done = graph.commits.len() < limit;
                state.take(graph);
            }
            Ok(Fetched::Page(graph)) => {
                state.error = None;
                state.done = graph.commits.len() < PAGE;
                state.take(graph);
            }
        }
    }
}

impl Commits {
    fn take(&mut self, graph: CommitGraph) {
        let (rows, end) = lanes(graph.commits, &self.end);
        self.widest = rows.iter().map(lane_count).fold(self.widest, usize::max);
        self.rows.extend(rows);
        self.end = end;
        self.version = Some(graph.version);
        self.worktrees = graph.worktrees;
    }
}

fn ago(cx: &gpui::App, time: i64, now: i64) -> String {
    let seconds = (now - time).max(0);
    let steps = [
        (
            365 * DAY,
            Str::CommitsAgoYear,
            Some(Str::CommitsAgoLastYear),
        ),
        (
            30 * DAY,
            Str::CommitsAgoMonth,
            Some(Str::CommitsAgoLastMonth),
        ),
        (7 * DAY, Str::CommitsAgoWeek, Some(Str::CommitsAgoLastWeek)),
        (DAY, Str::CommitsAgoDay, Some(Str::CommitsAgoLastDay)),
        (HOUR, Str::CommitsAgoHour, None),
        (MINUTE, Str::CommitsAgoMinute, None),
    ];
    for (unit, text, last) in steps {
        if seconds >= unit {
            let count = (seconds as f64 / unit as f64).round();
            if let (1.0, Some(last)) = (count, last) {
                return ui::say(cx, last).to_owned();
            }
            return ui::say_with(cx, text, &[("n", &count.to_string())]);
        }
    }
    ui::say(cx, Str::CommitsAgoNow).to_owned()
}

fn lanes_canvas(row: &Row, lanes: usize, colors: Colors) -> impl IntoElement {
    let drawn = drawing(row);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let at = |(x, y): (f32, f32)| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
            for line in &drawn.lines {
                let mut path = PathBuilder::stroke(px(1.0));
                for (i, p) in line.points.iter().enumerate() {
                    if i == 0 {
                        path.move_to(at(*p));
                    } else {
                        path.line_to(at(*p));
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, lane_color(&colors, line.color));
                }
            }
            let center = (strokes::lane_x(drawn.lane), LANE_WIDTH);
            let disc = |radius: f32, color: Rgba, window: &mut gpui::Window| {
                let corner = at((center.0 - radius, center.1 - radius));
                let bounds = Bounds::new(corner, size(px(radius * 2.0), px(radius * 2.0)));
                window.paint_quad(fill(bounds, color).corner_radii(px(radius)));
            };
            for radius in strokes::dot_radii(drawn.merge) {
                disc(radius + RING, colors.panel, window);
                disc(radius - RING, lane_color(&colors, drawn.color), window);
            }
        },
    )
    .w(px(strokes::width(lanes)))
    .h(px(ROW_HEIGHT))
    .flex_none()
}

fn ref_pill(
    name: String,
    tag: bool,
    folder: Option<(usize, String)>,
    color: Rgba,
    c: &Colors,
) -> Div {
    let pill = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.0))
        .h(px(REF_HEIGHT))
        .px(px(7.0))
        .rounded(px(REF_HEIGHT / 2.0))
        .border_1()
        .text_color(c.ink)
        .border_color(color)
        .bg(Hsla::from(color).opacity(REF_TINT))
        .font_family(fonts::TERMINAL_FAMILY)
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(11.0));
    let pill = if tag { pill.border_dashed() } else { pill };
    pill.children(folder.map(|(key, label)| {
        div()
            .id(("folder", key))
            .flex()
            .tooltip(tip(label))
            .child(icon(Glyph::Folder, FOLDER, c.ink))
    }))
    .child(name)
}

impl Dock {
    fn commit_row(&self, index: usize, c: &Colors, cx: &gpui::App) -> gpui::AnyElement {
        let state = &self.commits;
        let row = &state.rows[index];
        let commit = &row.commit;
        let color = lane_color(c, circle_of(row).color);
        let here: Vec<&Worktree> = state
            .worktrees
            .iter()
            .filter(|w| w.head == commit.hash)
            .collect();
        let kind = |w: &Worktree| {
            let text = if w.original {
                Str::WorktreeOriginal
            } else {
                Str::WorktreeLinked
            };
            ui::say(cx, text).to_owned()
        };
        let slot = index * 100;
        let refs = commit.refs.iter().enumerate().map(|(i, r)| {
            let (name, tag) = match r {
                GitRef::Branch(name) => (name, false),
                GitRef::Tag(name) => (name, true),
            };
            let folder = here
                .iter()
                .find(|w| !tag && w.branch.as_deref() == Some(name))
                .map(|w| (slot + i, kind(w)));
            ref_pill(name.clone(), tag, folder, color, c)
        });
        let detached = here
            .iter()
            .filter(|w| w.branch.is_none())
            .enumerate()
            .map(|(i, w)| {
                let key = slot + 50 + i;
                ref_pill("HEAD".to_owned(), false, Some((key, kind(w))), color, c)
            });
        div_row(index, c)
            .tooltip(tip(commit.hash.clone()))
            .child(lanes_canvas(row, state.widest, *c))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .children(refs)
                    .children(detached)
                    .child(div().min_w_0().truncate().child(commit.subject.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(AUTHOR_WIDTH))
                    .truncate()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(commit.author.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(TIME_WIDTH))
                    .truncate()
                    .text_right()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(ago(cx, commit.time, state.now)),
            )
            .into_any_element()
    }

    fn pick_repo(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let Some(base) = self.store.read(cx).base().map(str::to_owned) else {
            return;
        };
        let patch = serde_json::json!({ "repo": repo.0 });
        let store = self.store.clone();
        cx.spawn(async move |_, cx| {
            let saved = cx
                .background_spawn(async move { link::put_settings(&base, &patch) })
                .await;
            if saved.is_ok() {
                store.update(cx, |store, cx| store.reload(cx));
            }
        })
        .detach();
    }

    pub(super) fn commits_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let repos: Vec<(RepoId, String)> = self
            .store
            .read(cx)
            .snapshot()
            .map(|s| s.repos.iter().map(|r| (r.id, r.name.clone())).collect())
            .unwrap_or_default();
        let name = repos
            .iter()
            .find(|(id, _)| Some(*id) == self.commits.repo)
            .map(|(_, name)| name.clone());
        let picks = (repos.len() > 1).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(GAP))
                .px(px(PAD))
                .children(repos.into_iter().map(|(id, name)| {
                    let on = Some(id) == self.commits.repo;
                    let chip = ui::button(("commit-repo", id.0 as usize), cx);
                    let chip = if on {
                        chip.bg(c.accent_wash).border_color(c.accent)
                    } else {
                        chip
                    };
                    chip.child(name)
                        .on_click(cx.listener(move |dock, _, _, cx| dock.pick_repo(id, cx)))
                }))
        });
        let head = div()
            .flex()
            .gap(px(4.0))
            .px(px(PAD))
            .py(px(GAP))
            .text_size(px(SMALL))
            .text_color(c.ink_3)
            .children(name.map(|name| div().font_weight(FontWeight::BOLD).child(name)))
            .child(format!("\u{00b7} {}", ui::say(cx, Str::CommitsScope)));
        let panel = div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .children(picks);
        if let Some(error) = &self.commits.error {
            return panel.child(ui::note(error.clone(), true, cx));
        }
        if self.commits.version.is_none() {
            return panel.child(ui::note(
                ui::say(cx, Str::CommitsReading).to_owned(),
                false,
                cx,
            ));
        }
        let count = self.commits.rows.len();
        if count == 0 {
            return panel.child(ui::note(
                ui::say(cx, Str::CommitsEmpty).to_owned(),
                false,
                cx,
            ));
        }
        let handle = self.commit_bar.uniform_handle().unwrap_or_default();
        let list = uniform_list(
            ("commits", self.commits.epoch),
            count,
            cx.processor(move |dock, range: std::ops::Range<usize>, _, cx| {
                if range.end + LOAD_AHEAD >= count {
                    dock.commits_more(cx);
                }
                let c = ui::colors(cx);
                range
                    .map(|index| dock.commit_row(index, &c, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .size_full()
        .track_scroll(&handle);
        panel.child(self.commit_bar.wrap(list, false).flex_1().min_h_0())
    }
}

fn div_row(index: usize, c: &Colors) -> Stateful<Div> {
    let hover = c.surface_hover;
    div()
        .id(("commit", index))
        .flex()
        .w_full()
        .items_center()
        .gap(px(8.0))
        .h(px(ROW_HEIGHT))
        .pl(px(6.0))
        .pr(px(12.0))
        .text_size(px(ui::TEXT))
        .hover(move |style| style.bg(hover))
}
