use std::path::{Path, PathBuf};
use std::sync::Arc;

use api::ids::NodeId;
use gpui::{
    App, AppContext, ClipboardEntry, Context, Div, Entity, EventEmitter, Image, ImageFormat,
    InteractiveElement, MouseButton, ObjectFit, ParentElement, PathPromptOptions,
    StatefulInteractiveElement, Styled, StyledImage, div, img, px,
};

use super::dock::ui;
use super::tooltip::tip;
use super::viewer::Viewer;
use crate::store::Store;
use crate::strings::Str;

pub const MEGABYTE: u64 = 1024 * 1024;
pub const LIMIT_MEGABYTES: u64 = 32;
const THUMB: f32 = 34.0;

#[derive(Clone)]
enum Upload {
    Busy,
    Saved(String),
    Failed,
}

struct Attached {
    id: u32,
    name: String,
    picture: Arc<Image>,
    state: Upload,
}

pub enum PicturesEvent {
    Settled,
    Refused(String),
    Other(String, Vec<u8>),
}

enum Unreadable {
    Folder(String),
    Big(String, u64),
    Io(String),
}

pub struct Pictures {
    store: Entity<Store>,
    node: NodeId,
    list: Vec<Attached>,
    next: u32,
}

impl EventEmitter<PicturesEvent> for Pictures {}

fn image_format(name: &str) -> Option<ImageFormat> {
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "bmp" => ImageFormat::Bmp,
        _ => return None,
    })
}

fn read_file(path: &Path) -> Result<(String, Vec<u8>), Unreadable> {
    let name = path
        .file_name()
        .map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
    let meta = std::fs::metadata(path).map_err(|e| Unreadable::Io(e.to_string()))?;
    if meta.is_dir() {
        return Err(Unreadable::Folder(name));
    }
    if meta.len() > LIMIT_MEGABYTES * MEGABYTE {
        return Err(Unreadable::Big(name, meta.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| Unreadable::Io(e.to_string()))?;
    Ok((name, bytes))
}

pub fn too_big(name: &str, bytes: u64, cx: &App) -> String {
    ui::say_with(
        cx,
        Str::TerminalTooBig,
        &[
            ("name", name),
            ("size", &bytes.div_ceil(MEGABYTE).to_string()),
            ("limit", &LIMIT_MEGABYTES.to_string()),
        ],
    )
}

impl Pictures {
    pub fn new(store: Entity<Store>, node: NodeId) -> Self {
        Self {
            store,
            node,
            list: Vec::new(),
            next: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn busy(&self) -> bool {
        self.list
            .iter()
            .any(|image| matches!(image.state, Upload::Busy))
    }

    pub fn saved(&self) -> Vec<(u32, String)> {
        self.list
            .iter()
            .filter_map(|image| match &image.state {
                Upload::Saved(path) => Some((image.id, path.clone())),
                _ => None,
            })
            .collect()
    }

    pub fn forget(&mut self, ids: &[u32], cx: &mut Context<Self>) {
        self.list.retain(|image| !ids.contains(&image.id));
        cx.notify();
    }

    pub fn take_clipboard(&mut self, entries: Vec<ClipboardEntry>, cx: &mut Context<Self>) {
        for entry in entries {
            match entry {
                ClipboardEntry::Image(image) => {
                    let name = format!("image.{}", image.format.extension());
                    self.attach(name, image.format, image.bytes, cx);
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    self.take_paths(paths.paths().to_vec(), cx);
                }
                ClipboardEntry::String(_) => {}
            }
        }
    }

    pub fn take_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            let read = cx.background_spawn(async move { read_file(&path) });
            cx.spawn(async move |this, cx| {
                let read = read.await;
                let _ = this.update(cx, |pictures, cx| match read {
                    Ok((name, bytes)) => match image_format(&name) {
                        Some(format) => pictures.attach(name, format, bytes, cx),
                        None => cx.emit(PicturesEvent::Other(name, bytes)),
                    },
                    Err(error) => {
                        let text = match error {
                            Unreadable::Folder(name) => {
                                ui::say_with(cx, Str::TerminalFolder, &[("name", &name)])
                            }
                            Unreadable::Big(name, bytes) => too_big(&name, bytes, cx),
                            Unreadable::Io(text) => text,
                        };
                        cx.emit(PicturesEvent::Refused(text));
                    }
                });
            })
            .detach();
        }
    }

    pub fn pick_files(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                let _ = this.update(cx, |pictures, cx| pictures.take_paths(paths, cx));
            }
        })
        .detach();
    }

    fn attach(
        &mut self,
        name: String,
        format: ImageFormat,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        if bytes.len() as u64 > LIMIT_MEGABYTES * MEGABYTE {
            let text = too_big(&name, bytes.len() as u64, cx);
            cx.emit(PicturesEvent::Refused(text));
            return;
        }
        let id = self.next;
        self.next += 1;
        self.list.push(Attached {
            id,
            name: name.clone(),
            picture: Arc::new(Image::from_bytes(format, bytes.clone())),
            state: Upload::Busy,
        });
        let node = self.node;
        let task = self.store.update(cx, |store, cx| {
            store.save_paste(node, name, format.mime_type(), bytes, cx)
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let saved = task.await;
            let _ = this.update(cx, |pictures, cx| pictures.finished(id, saved, cx));
        })
        .detach();
    }

    fn finished(&mut self, id: u32, saved: Result<String, String>, cx: &mut Context<Self>) {
        let Some(image) = self.list.iter_mut().find(|i| i.id == id) else {
            return;
        };
        image.state = match &saved {
            Ok(path) => Upload::Saved(path.clone()),
            Err(_) => Upload::Failed,
        };
        if let Err(error) = saved {
            cx.emit(PicturesEvent::Refused(error));
        }
        cx.notify();
        cx.emit(PicturesEvent::Settled);
    }

    fn remove(&mut self, id: u32, cx: &mut Context<Self>) {
        self.list.retain(|image| image.id != id);
        cx.notify();
    }
}

pub fn chips(
    pictures: &Entity<Pictures>,
    viewer: &Entity<Viewer>,
    id: &'static str,
    scope: u32,
    z: f32,
    cx: &mut App,
) -> Option<Div> {
    let c = ui::colors(cx);
    let list = &pictures.read(cx).list;
    if list.is_empty() {
        return None;
    }
    let chips: Vec<Div> = list
        .iter()
        .map(|image| {
            let attached = image.id;
            let state = match image.state {
                Upload::Busy => Some(Str::ComposerUploading),
                Upload::Failed => Some(Str::ComposerFailed),
                Upload::Saved(_) => None,
            };
            let picture = image.picture.clone();
            let name = image.name.clone();
            let open = viewer.clone();
            let zoomed = picture.clone();
            let owner = pictures.clone();
            div()
                .flex()
                .items_center()
                .gap(px(4.0 * z))
                .p(px(3.0 * z))
                .rounded(px(6.0 * z))
                .border_1()
                .border_color(if matches!(image.state, Upload::Failed) {
                    c.err
                } else {
                    c.line
                })
                .bg(c.surface)
                .text_size(px(12.0 * z))
                .text_color(c.ink_3)
                .child(
                    div()
                        .id((id, ((scope as usize) << 20) | attached as usize))
                        .cursor_pointer()
                        .tooltip(tip(ui::say_with(
                            cx,
                            Str::InboxZoomImage,
                            &[("name", &name)],
                        )))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            let picture = zoomed.clone();
                            open.update(cx, |viewer, cx| viewer.open(picture, window, cx));
                        })
                        .child(
                            img(picture)
                                .size(px(THUMB * z))
                                .rounded(px(4.0 * z))
                                .object_fit(ObjectFit::Cover),
                        ),
                )
                .children(state.map(|s| ui::say(cx, s)))
                .children((!matches!(image.state, Upload::Busy)).then(|| {
                    div()
                        .id((id, ((scope as usize) << 20) | (1 << 19) | attached as usize))
                        .size(px(20.0 * z))
                        .rounded(px(4.0 * z))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(move |s| s.bg(c.surface_hover).text_color(c.ink))
                        .tooltip(tip(format!(
                            "{}: {}",
                            ui::say(cx, Str::InboxRemoveImage),
                            name
                        )))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            owner.update(cx, |pictures, cx| pictures.remove(attached, cx))
                        })
                        .child("\u{00d7}")
                }))
        })
        .collect();
    Some(div().flex().flex_wrap().gap(px(6.0 * z)).children(chips))
}
