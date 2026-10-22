//! Worker-backed archive browser and folder-size details for Files.
use super::*;
use scarlet_ui::ListView;

pub(super) const WINDOW_KEY: &str = "details";
static RESULTS: Mutex<Vec<DetailResult>> = Mutex::new(Vec::new());
struct DetailResult {
    instance: u64,
    generation: u64,
    output: std::result::Result<DetailOutput, String>,
}
enum DetailOutput {
    Archive(archive::Index),
    Usage(usage::Usage),
    Extracted(PathBuf),
}
impl FilerApp {
    pub(super) fn show_details(&self) {
        let path = self
            .selected_entry()
            .map(|entry| entry.path)
            .unwrap_or_else(|| self.current_path.get());
        let archive = archive::supported(Path::new(&path));
        self.show_details_path(path, archive);
    }
    pub(super) fn show_details_path(&self, path: String, archive: bool) {
        if self.details_busy.get() || self.operation_busy.get() {
            self.status
                .set("Wait for the current file operation, or cancel it in File Details".into());
            return;
        }
        self.details_archive.set(archive);
        self.details_path.set(path.clone());
        self.details_folder.set(String::new());
        self.details_index.set(None);
        self.details_entries.set(Vec::new());
        self.details_selected.set(None);
        self.details_last_click.set(None);
        self.details_usage.set(None);
        self.details_usage_rows.set(Vec::new());
        self.details_name
            .set(archive::suggested_folder(Path::new(&path)));
        self.details_destination.set(
            Path::new(&path)
                .parent()
                .unwrap_or_else(|| Path::new(ROOT_PATH))
                .to_string_lossy()
                .into_owned(),
        );
        self.details_status.set(
            if archive {
                "Reading archive…"
            } else {
                "Calculating logical file sizes…"
            }
            .into(),
        );
        self.details_close_requested.set(false);
        let (instance, generation, cancelled) = self.begin_details_task();
        open_window(WINDOW_KEY);
        if let Err(error) = workers::spawn("files-details", move || {
            let output = if archive {
                archive::inspect(Path::new(&path), &cancelled).map(DetailOutput::Archive)
            } else {
                usage::measure(Path::new(&path), &cancelled).map(DetailOutput::Usage)
            };
            mutex_lock(&RESULTS).push(DetailResult {
                instance,
                generation,
                output,
            });
        }) {
            self.details_spawn_failed(error);
        }
    }
    fn begin_details_task(&self) -> (u64, u64, Arc<AtomicBool>) {
        self.details_busy.set(true);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.details_cancel.set(Some(cancelled.clone()));
        let generation = self.details_generation.get().wrapping_add(1);
        self.details_generation.set(generation);
        (self.directory_read_instance.get(), generation, cancelled)
    }
    fn details_spawn_failed(&self, error: std::io::Error) {
        self.details_busy.set(false);
        self.details_cancel.set(None);
        self.details_status
            .set(format!("Cannot start file details: {error}"));
    }
    pub(super) fn cancel_details(&self, close: bool) {
        if self.details_busy.get() {
            if let Some(cancelled) = self.details_cancel.get() {
                cancelled.store(true, AtomicOrdering::Relaxed);
            }
            self.details_close_requested.set(close);
            self.details_status
                .set("Cancelling… waiting for the current file step to finish".into());
        } else if close {
            dismiss_window(WINDOW_KEY);
        }
    }
    fn extract_details(&self) {
        if self.details_busy.get() || self.operation_busy.get() {
            return;
        }
        let Some(index) = self.details_index.get() else {
            return;
        };
        let name = self.details_name.get();
        if let Err(error) = operations::validate_name(&name) {
            self.details_status.set(error);
            return;
        }
        let archive = self.details_path.get();
        let directory = self.details_destination.get();
        let (instance, generation, cancelled) = self.begin_details_task();
        self.details_status
            .set("Validating and extracting into a new folder…".into());
        if let Err(error) = workers::spawn("files-extract", move || {
            let output = std::fs::canonicalize(&directory)
                .map_err(|error| format!("Cannot open destination folder: {error}"))
                .and_then(|directory| {
                    archive::extract(
                        Path::new(&archive),
                        &directory,
                        &name,
                        &index,
                        &cancelled,
                    )
                })
                .map(DetailOutput::Extracted);
            mutex_lock(&RESULTS).push(DetailResult {
                instance,
                generation,
                output,
            });
        }) {
            self.details_spawn_failed(error);
        }
    }
    pub(super) fn process_details_results(&self) {
        let results: Vec<_> = mutex_lock(&RESULTS).drain(..).collect();
        for result in results {
            if result.instance != self.directory_read_instance.get()
                || result.generation != self.details_generation.get()
            {
                continue;
            }
            self.details_busy.set(false);
            self.details_cancel.set(None);
            match result.output {
                Ok(DetailOutput::Archive(index)) => {
                    self.details_status.set(format!(
                        "{} archive entries · {} expanded · double-click a folder or press Enter",
                        index.entries.len(),
                        format_size(index.total_bytes)
                    ));
                    self.details_entries.set(index.children(""));
                    self.details_index.set(Some(Arc::new(index)));
                }
                Ok(DetailOutput::Usage(usage)) => {
                    self.details_status.set(
                        if usage.incomplete.is_some() {
                            "Partial calculation — some items could not be measured"
                        } else {
                            "Size calculation complete"
                        }
                        .into(),
                    );
                    self.details_usage_rows.set(usage.largest.clone());
                    self.details_usage.set(Some(usage));
                }
                Ok(DetailOutput::Extracted(path)) => {
                    let message = format!("Extracted to {}", path.display());
                    self.details_status.set(message.clone());
                    self.refresh_message.set(Some(message));
                    self.refresh();
                }
                Err(error) => self.details_status.set(error),
            }
            if self.details_close_requested.get() {
                self.details_close_requested.set(false);
                dismiss_window(WINDOW_KEY);
            }
        }
    }
    fn details_navigate(&self, folder: String) {
        if let Some(index) = self.details_index.get() {
            self.details_entries.set(index.children(&folder));
            self.details_folder.set(folder);
            self.details_selected.set(None);
            self.details_last_click.set(None);
        }
    }
    fn details_up(&self) {
        let folder = self.details_folder.get();
        self.details_navigate(
            Path::new(&folder)
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_string_lossy()
                .into_owned(),
        );
    }
    fn details_open_selected(&self) {
        if let Some(entry) = self
            .details_selected
            .get()
            .and_then(|index| self.details_entries.get().get(index).cloned())
        {
            if entry.is_directory {
                self.details_navigate(entry.path);
            } else {
                self.details_status.set(format!(
                    "{} · {} — extract the archive to open this file",
                    entry.path,
                    format_size(entry.size)
                ));
            }
        }
    }
    fn details_key(&self, event: KeyEvent) -> bool {
        let KeyEvent::Pressed { keycode, modifiers } = event else {
            return false;
        };
        if keycode == KeyCode::Escape {
            self.cancel_details(true);
            return true;
        }
        if modifiers.alt && keycode == KeyCode::Up {
            self.details_up();
            return true;
        }
        if !self.details_focused.get() {
            return false;
        }
        let length = self.details_entries.get().len();
        if length == 0 {
            return false;
        }
        let selected = self.details_selected.get().unwrap_or(0).min(length - 1);
        let columns = self.details_columns.get().max(1);
        let next = match keycode {
            KeyCode::Left => selected.saturating_sub(1),
            KeyCode::Right => (selected + 1).min(length - 1),
            KeyCode::Up => selected.saturating_sub(columns),
            KeyCode::Down => (selected + columns).min(length - 1),
            KeyCode::Home => 0,
            KeyCode::End => length - 1,
            KeyCode::Enter => {
                self.details_open_selected();
                return true;
            }
            KeyCode::Backspace => {
                self.details_up();
                return true;
            }
            _ => return false,
        };
        self.details_selected.set(Some(next));
        true
    }
    pub(super) fn details_page(&self) -> impl View + Clone + use<> {
        let close = self.clone();
        let cancel = self.clone();
        let key = self.clone();
        let width_state = self.details_width.clone();
        let width = (self.details_width.get() - 28.0).max(600.0);
        let path = self.details_path.get();
        let name = Path::new(&path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let palette = ColorPalette::default();
        let content = if self.details_archive.get() {
            Either::A(self.archive_page())
        } else {
            Either::B(self.usage_page())
        };
        let cancel_button = if self.details_busy.get() {
            Either::A(
                Button::new("Cancel Operation").on_click(move || cancel.cancel_details(false)),
            )
        } else {
            Either::B(Spacer::new().frame_width(0.0))
        };
        vstack! {
            hstack! {
                icon_for_entry(&name, !self.details_archive.get()).frame(38.0, 38.0),
                vstack! {
                    Text::new(if self.details_archive.get() { "Archive contents" } else { "Folder size" }).font_size(20.0),
                    Text::new(wrap_display_text(&path, width - 50.0, 12.0)).font_size(12.0).color(palette.text_secondary()),
                }.alignment(Alignment::Leading).spacing(4.0),
                Spacer::new(),
            }.spacing(12.0),
            Divider::new(),
            content,
            Divider::new(),
            hstack! {
                Text::new(wrap_display_text(&self.details_status.get(), width - 160.0, 12.0)).font_size(12.0),
                Spacer::new(),
                cancel_button,
                Button::new("Close").on_click(move || close.cancel_details(true)),
            }.spacing(8.0),
        }.alignment(Alignment::Leading).spacing(12.0).padding(16.0)
            .frame(f32::INFINITY, f32::INFINITY)
            .on_geometry_change(|geometry| geometry.size().width, move |width| width_state.set(width))
            .on_key(move |event| key.details_key(event))
    }
    fn usage_page(&self) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let width = (self.details_width.get() - 64.0).max(550.0);
        if let Some(usage) = self.details_usage.get() {
            let row_width = (width - 120.0).max(250.0);
            let rows = ListView::new(
                self.details_usage_rows.clone(),
                self.details_selected.clone(),
                48.0,
                move |_, (name, size): (String, u64), _| {
                    vstack! {
                        hstack! {
                            Text::new(wrap_display_text(&name, row_width, 13.0)).font_size(13.0).frame(f32::INFINITY, 38.0),
                            Text::new(format_size(size)).font_size(13.0).frame_width(90.0),
                        }.spacing(12.0),
                        Divider::new(),
                    }.alignment(Alignment::Leading).padding(4.0)
                },
            );
            let partial = usage
                .incomplete
                .as_ref()
                .map(|reason| format!("Partial result: {reason}. {} items skipped.", usage.skipped))
                .unwrap_or_default();
            Either::A(vstack! {
                hstack! {
                    vstack! {
                        Text::new(format_size(usage.bytes)).font_size(30.0),
                        Text::new(format!("{} bytes in files", usage.bytes)).font_size(12.0).color(palette.text_secondary()),
                    }.alignment(Alignment::Leading).spacing(4.0),
                    Spacer::new(),
                    vstack! {
                        Text::new(format!("{} file{} · {} folder{}", usage.files, if usage.files == 1 { "" } else { "s" }, usage.folders, if usage.folders == 1 { "" } else { "s" })).font_size(14.0),
                        Text::new(format!("{} symbolic link{}", usage.links, if usage.links == 1 { "" } else { "s" })).font_size(12.0).color(palette.text_secondary()),
                    }.alignment(Alignment::Leading).spacing(4.0),
                }.padding(16.0).background(palette.surface_variant()).clip_radius(8.0),
                Text::new("Largest items in this folder").font_size(15.0),
                rows,
                Text::new(wrap_display_text("Logical file sizes. Links are not followed; hard-linked paths are counted separately.", width, 12.0)).font_size(12.0).color(palette.text_secondary()),
                Text::new(wrap_display_text(&partial, width, 12.0)).font_size(12.0),
            }.alignment(Alignment::Leading).spacing(12.0).frame(f32::INFINITY, f32::INFINITY))
        } else {
            Either::B(vstack! {
                Spacer::new(),
                Text::new(if self.details_busy.get() { "Calculating folder size…" } else { "No size information available" }).font_size(16.0).color(palette.text_secondary()),
                Spacer::new(),
            }.frame(f32::INFINITY, f32::INFINITY))
        }
    }
    fn archive_page(&self) -> impl View + Clone + use<> {
        let up = self.clone();
        let root = self.clone();
        let extract = self.clone();
        let cells = self.clone();
        let columns = self.details_columns.clone();
        let grid=GridView::new(self.details_entries.clone(),self.details_selected.clone(),5,110.0,move |index,entry:archive::Entry,selected| {
            let app=cells.clone();
            let name=Path::new(&entry.path).file_name().unwrap_or_default().to_string_lossy().into_owned();
            let background=if selected==Some(index) {ColorPalette::default().primary_light().with_opacity(0.2)} else {Color::CLEAR};
            vstack! {
                icon_for_entry(&name,entry.is_directory).frame(64.0,48.0),
                Text::new(file_name_label(&name)).font_size(12.0),
                Text::new(if entry.is_directory {"Folder".into()} else {format_size(entry.size)}).font_size(10.0),
            }.alignment(Alignment::Center).padding(6.0).frame(f32::INFINITY,110.0)
                .background(background).clip_radius(6.0)
                .on_click(move || {
                    let now=Instant::now();
                    let double=app.details_last_click.get().is_some_and(|(last,at)|last==index && now.duration_since(at)<=DOUBLE_CLICK_INTERVAL);
                    app.details_selected.set(Some(index));
                    app.details_last_click.set(if double {None} else {Some((index,now))});
                    if double {app.details_open_selected();}
                })
        }).spacing(10.0).minimum_cell_width(140.0).reveal_selection(true)
            .item_key(|entry|entry.path.clone().into())
            .on_geometry_change(|geometry|(((geometry.size().width+10.0)/150.0).floor().max(1.0) as usize).min(5),move |count|columns.set(count))
            .focusable(self.details_focused.clone());
        vstack! {
            hstack! {
                Button::new("Archive Root").on_click(move ||root.details_navigate(String::new())),
                Button::new("Up").enabled(!self.details_folder.get().is_empty()).on_click(move ||up.details_up()),
                Text::new(format!("/{}",self.details_folder.get())).font_size(12.0),
                Spacer::new(),
            }.spacing(8.0),
            grid,
            Divider::new(),
            Text::new("Extract archive").font_size(15.0),
            Text::new("Choose a new folder. Existing files are kept.").font_size(12.0).color(ColorPalette::default().text_secondary()),
            hstack! {
                Text::new("Destination").font_size(12.0).frame_width(85.0),
                TextField::new(self.details_destination.clone()).placeholder("Existing destination folder").frame(f32::INFINITY,34.0),
            }.spacing(8.0),
            hstack! {
                Text::new("New folder").font_size(12.0).frame_width(85.0),
                TextField::new(self.details_name.clone()).placeholder("New folder name").frame(f32::INFINITY,34.0),
                Button::new("Extract").enabled(self.details_index.get().is_some()&&!self.details_busy.get()).on_click(move ||extract.extract_details()),
            }.spacing(8.0),
        }.alignment(Alignment::Leading).spacing(8.0).frame(f32::INFINITY,f32::INFINITY)
    }
}
