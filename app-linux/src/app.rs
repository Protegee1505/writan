//! The GTK4 shell itself: window, three-pane layout, callbacks.
//!
//! UI-thin on purpose (ADR-0004): every operation delegates to
//! `writan-core`; nothing here touches vault files directly.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use adw::prelude::*;
use sourceview5::prelude::*;

use writan_core::{Note, NoteId, SearchQuery, VaultEvent, VaultWatcher, Workspace};

use crate::config;
use crate::preview::Preview;

/// How often the main loop drains the watcher's event channel.
const WATCH_POLL: Duration = Duration::from_millis(200);

struct State {
    workspace: Option<Workspace>,
    /// Note ids in list order (row index → note).
    rows: Vec<NoteId>,
    current: Option<Note>,
    /// True while the editor buffer is being filled programmatically, so the
    /// change signal must not mark the note dirty.
    loading: bool,
    /// True while `refresh` re-selects the open note; the list signal must
    /// not reload it (that would move the cursor while typing).
    suppress_select: bool,
    /// The buffer differs from the file on disk.
    dirty: bool,
    /// Pending debounced autosave (ADR-0003 `editor.autosaveMs`).
    autosave: Option<gtk::glib::SourceId>,
    /// Pending clear of the transient "Saved" label.
    status_timer: Option<gtk::glib::SourceId>,
    watcher: Option<VaultWatcher>,
    autosave_ms: u64,
    config_path: PathBuf,
    data_dir: PathBuf,
}

/// Widget handles cloned into callbacks.
#[derive(Clone)]
struct Ui {
    window: adw::ApplicationWindow,
    toast: adw::ToastOverlay,
    stack: gtk::Stack,
    list: gtk::ListBox,
    search: gtk::SearchEntry,
    status: gtk::Label,
    editor: sourceview5::View,
    preview: Preview,
}

pub fn run() -> gtk::glib::ExitCode {
    let application = adw::Application::builder()
        .application_id("com.writan.desktop")
        .build();
    application.connect_activate(build);
    application.run()
}

fn build(application: &adw::Application) {
    let started = Instant::now();
    load_css();

    let config_path = config::default_config_path();
    let state = Rc::new(RefCell::new(State {
        workspace: None,
        rows: Vec::new(),
        current: None,
        loading: false,
        suppress_select: false,
        dirty: false,
        autosave: None,
        status_timer: None,
        watcher: None,
        autosave_ms: config::load_autosave_ms(&config_path),
        config_path,
        data_dir: config::state_dir(),
    }));

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Writan")
        .default_width(1000)
        .default_height(700)
        .build();

    // Header bar: open folder, save status, title, quick actions.
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::new(Some("Writan"))));
    let open_button = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Choose notes folder…")
        .build();
    let status = gtk::Label::builder()
        .css_classes(["dim-label", "caption"])
        .build();
    let new_button = gtk::Button::builder()
        .icon_name("document-new-symbolic")
        .tooltip_text("New note (Ctrl+N)")
        .build();
    let preview_toggle = gtk::ToggleButton::builder()
        .icon_name("view-reveal-symbolic")
        .tooltip_text("Toggle preview")
        .active(true)
        .build();
    header.pack_start(&open_button);
    header.pack_start(&status);
    header.pack_end(&preview_toggle);
    header.pack_end(&new_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    // Sidebar: search + note list.
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search notes")
        .margin_start(10)
        .margin_end(10)
        .margin_top(10)
        .margin_bottom(6)
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("navigation-sidebar");
    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.set_width_request(280);
    sidebar.append(&search);
    sidebar.append(&sidebar_scroll);

    // Editor: GtkSourceView with Markdown highlighting.
    let buffer = sourceview5::Buffer::new(None);
    if let Some(language) = sourceview5::LanguageManager::new().language("markdown") {
        buffer.set_language(Some(&language));
    }

    // Editor theme follows the system light/dark preference.
    {
        apply_style_scheme(&buffer);
        let buffer = buffer.clone();
        adw::StyleManager::default().connect_dark_notify(move |_| apply_style_scheme(&buffer));
    }
    let editor = sourceview5::View::with_buffer(&buffer);
    editor.set_monospace(true);
    editor.set_wrap_mode(gtk::WrapMode::WordChar);
    editor.set_highlight_current_line(true);
    editor.set_left_margin(24);
    editor.set_right_margin(24);
    editor.set_top_margin(16);
    editor.set_bottom_margin(16);
    let editor_scroll = gtk::ScrolledWindow::builder()
        .child(&editor)
        .hexpand(true)
        .vexpand(true)
        .build();

    // Preview pane.
    let preview = Preview::new();
    let preview_scroll = gtk::ScrolledWindow::builder()
        .child(&preview.view)
        .hexpand(true)
        .vexpand(true)
        .build();
    preview_scroll.add_css_class("preview-pane");

    let inner = gtk::Paned::new(gtk::Orientation::Horizontal);
    inner.set_start_child(Some(&editor_scroll));
    inner.set_end_child(Some(&preview_scroll));
    inner.set_resize_start_child(true);
    inner.set_resize_end_child(false);
    inner.set_position(520);

    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&sidebar));
    split.set_end_child(Some(&inner));
    split.set_resize_start_child(false);
    split.set_resize_end_child(true);
    split.set_position(280);

    // Welcome page (no vault yet).
    let welcome = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    welcome.append(
        &gtk::Label::builder()
            .label("Writan")
            .css_classes(["title-1"])
            .build(),
    );
    welcome.append(
        &gtk::Label::builder()
            .label("A folder of Markdown files is the vault.")
            .css_classes(["dim-label"])
            .build(),
    );
    let welcome_button = gtk::Button::builder()
        .label("Choose notes folder…")
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    welcome.append(&welcome_button);

    let stack = gtk::Stack::new();
    stack.add_named(&split, Some("content"));
    stack.add_named(&welcome, Some("welcome"));

    let toast = adw::ToastOverlay::new();
    toast.set_child(Some(&stack));
    toolbar.set_content(Some(&toast));
    window.set_content(Some(&toolbar));

    let ui = Ui {
        window: window.clone(),
        toast: toast.clone(),
        stack: stack.clone(),
        list: list.clone(),
        search: search.clone(),
        status: status.clone(),
        editor: editor.clone(),
        preview: preview.clone(),
    };

    // Selection loads the note; a programmatic re-selection of the open note
    // is ignored so typing is never interrupted.
    {
        let state = state.clone();
        let ui = ui.clone();
        list.connect_row_selected(move |_, row| {
            let Some(row) = row else {
                return;
            };
            let index = row.index();
            if index < 0 {
                return;
            }
            let (id, skip) = {
                let st = state.borrow();
                let id = st.rows.get(index as usize).copied();
                let same = st.current.as_ref().map(|note| note.meta.id) == id;
                (id, st.suppress_select || same)
            };
            if !skip && let Some(id) = id {
                load_note(&state, &ui, id);
            }
        });
    }

    // Search re-fills the list.
    {
        let state = state.clone();
        let ui = ui.clone();
        search.connect_search_changed(move |_| refresh(&state, &ui));
    }

    // Editor → preview, dirty state, debounced autosave.
    {
        let state = state.clone();
        let ui = ui.clone();
        buffer.connect_changed(move |buffer| {
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            preview.render(&text);
            if !state.borrow().loading {
                schedule_autosave(&state, &ui);
            }
        });
    }

    // Preview visibility.
    {
        let preview_scroll = preview_scroll.clone();
        preview_toggle
            .connect_toggled(move |toggle| preview_scroll.set_visible(toggle.is_active()));
    }

    // New note.
    {
        let state = state.clone();
        let ui = ui.clone();
        let create = move || {
            let created = {
                let mut st = state.borrow_mut();
                st.workspace
                    .as_mut()
                    .and_then(|ws| ws.create_note("Untitled", &[]).ok())
            };
            let Some(note) = created else {
                ui.toast
                    .add_toast(adw::Toast::new("Open a notes folder first"));
                return;
            };
            ui.search.set_text("");
            refresh(&state, &ui);
            let position = state
                .borrow()
                .rows
                .iter()
                .position(|id| *id == note.meta.id);
            match position.and_then(|p| ui.list.row_at_index(p as i32)) {
                Some(row) => {
                    ui.list.select_row(Some(&row));
                }
                None => load_note(&state, &ui, note.meta.id),
            }
            ui.editor.grab_focus();
        };
        let create2 = create.clone();
        new_button.connect_clicked(move |_| create());
        let new_action = gtk::gio::SimpleAction::new("new-note", None);
        new_action.connect_activate(move |_, _| create2());
        application.add_action(&new_action);
        application.set_accels_for_action("app.new-note", &["<Ctrl>N"]);
    }

    // Choose folder (header button and welcome page).
    {
        let state_for_open = state.clone();
        let ui_for_open = ui.clone();
        let state_for_welcome = state.clone();
        let ui_for_welcome = ui.clone();
        open_button.connect_clicked(move |_| choose_folder(&state_for_open, &ui_for_open));
        welcome_button.connect_clicked(move |_| choose_folder(&state_for_welcome, &ui_for_welcome));
    }

    // Save (Ctrl+S).
    let save_action = gtk::gio::SimpleAction::new("save", None);
    {
        let state = state.clone();
        let ui = ui.clone();
        save_action.connect_activate(move |_, _| {
            save_now(&state, &ui, true);
        });
    }
    application.add_action(&save_action);
    application.set_accels_for_action("app.save", &["<Ctrl>S"]);

    // Focus search (Ctrl+F).
    let find_action = gtk::gio::SimpleAction::new("find", None);
    {
        let search = search.clone();
        find_action.connect_activate(move |_, _| {
            search.grab_focus();
        });
    }
    application.add_action(&find_action);
    application.set_accels_for_action("app.find", &["<Ctrl>F"]);

    // Save when the window closes.
    {
        let state = state.clone();
        let ui = ui.clone();
        window.connect_close_request(move |_| {
            save_now(&state, &ui, false);
            gtk::glib::Propagation::Proceed
        });
    }

    // Drain the vault watcher on the main loop (parity with the Tauri
    // shell's `vault-changed` events).
    {
        let state = state.clone();
        let ui = ui.clone();
        gtk::glib::timeout_add_local(WATCH_POLL, move || {
            let batch = {
                let st = state.borrow();
                match st.watcher.as_ref() {
                    Some(watcher) => watcher.try_recv(),
                    None => return gtk::glib::ControlFlow::Continue,
                }
            };
            match batch {
                Ok(events) => handle_external(&state, &ui, events),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => state.borrow_mut().watcher = None,
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    // Open the previous vault (env override for the spike).
    let initial = std::env::var_os("WRITAN_VAULT")
        .map(PathBuf::from)
        .or_else(|| config::load_vault_path(&state.borrow().config_path));
    if let Some(path) = initial
        && open_vault(&state, &path).is_ok()
    {
        stack.set_visible_child_name("content");
        refresh(&state, &ui);
    }

    // Spike measurement hooks: WRITAN_MEASURE=1 prints timings and exits.
    if std::env::var_os("WRITAN_MEASURE").is_some() {
        let measured = std::rc::Rc::new(std::cell::Cell::new(false));
        let window_for_map = window.clone();
        window.connect_map(move |_| {
            if measured.replace(true) {
                return;
            }
            eprintln!(
                "writan-measure cold-start-ms: {}",
                started.elapsed().as_millis()
            );
            let window = window_for_map.clone();
            gtk::glib::timeout_add_seconds_local(3, move || {
                eprintln!("writan-measure idle-rss-kb: {}", read_vm_rss_kb());
                if let Some(application) = window.application() {
                    application.quit();
                }
                gtk::glib::ControlFlow::Break
            });
        });
    }

    window.present();
}

/// Show the folder picker and open the chosen vault.
fn choose_folder(state: &Rc<RefCell<State>>, ui: &Ui) {
    let dialog = gtk::FileDialog::builder()
        .title("Choose notes folder")
        .build();
    let state = state.clone();
    let ui = ui.clone();
    let parent = ui.window.clone();
    dialog.select_folder(Some(&parent), gtk::gio::Cancellable::NONE, move |result| {
        let Ok(folder) = result else {
            return;
        };
        let Some(path) = folder.path() else {
            return;
        };
        match open_vault(&state, &path) {
            Ok(()) => {
                ui.stack.set_visible_child_name("content");
                refresh(&state, &ui);
            }
            Err(error) => {
                ui.toast
                    .add_toast(adw::Toast::new(&format!("Could not open vault: {error}")));
            }
        }
    });
}

fn open_vault(state: &Rc<RefCell<State>>, path: &Path) -> Result<(), String> {
    let mut st = state.borrow_mut();
    let mut workspace = Workspace::open(path, &st.data_dir).map_err(|e| e.to_string())?;
    workspace.reconcile().map_err(|e| e.to_string())?;

    // Swap the watcher with the vault; dropping the old one stops its thread.
    st.watcher = None;
    let watch_root = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match VaultWatcher::spawn(&watch_root) {
        Ok(watcher) => st.watcher = Some(watcher),
        Err(error) => eprintln!("writan: failed to watch {}: {error}", watch_root.display()),
    }

    st.workspace = Some(workspace);
    st.current = None;
    st.dirty = false;
    st.rows.clear();
    if let Err(error) = config::save_vault_path(&st.config_path, path) {
        eprintln!("writan: could not save config: {error}");
    }
    Ok(())
}

/// Rebuild the note list for the current search; keep the open note selected
/// when it is still listed, otherwise leave the list unselected (the editor
/// keeps showing it, exactly like the Tauri shell).
fn refresh(state: &Rc<RefCell<State>>, ui: &Ui) {
    while let Some(child) = ui.list.first_child() {
        ui.list.remove(&child);
    }
    let query = ui.search.text();
    let entries: Vec<(NoteId, String, String)> = {
        let st = state.borrow();
        let Some(workspace) = st.workspace.as_ref() else {
            return;
        };
        let query = query.trim();
        if query.is_empty() {
            workspace
                .list(500)
                .unwrap_or_default()
                .into_iter()
                .map(|note| (note.id, note.title, note.rel_path))
                .collect()
        } else {
            workspace
                .search(&SearchQuery {
                    text: query.to_string(),
                    limit: Some(200),
                    ..SearchQuery::default()
                })
                .unwrap_or_default()
                .into_iter()
                .map(|hit| {
                    let subtitle = if hit.snippet.is_empty() {
                        hit.rel_path
                    } else {
                        hit.snippet
                    };
                    (hit.id, hit.title, subtitle)
                })
                .collect()
        }
    };
    let mut rows = Vec::with_capacity(entries.len());
    for (id, title, subtitle) in &entries {
        let row = gtk::ListBoxRow::new();
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
        cell.set_margin_start(10);
        cell.set_margin_end(10);
        cell.set_margin_top(6);
        cell.set_margin_bottom(6);
        let title_label = gtk::Label::new(Some(title));
        title_label.set_xalign(0.0);
        title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let subtitle_label = gtk::Label::new(Some(subtitle));
        subtitle_label.set_xalign(0.0);
        subtitle_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle_label.add_css_class("dim-label");
        subtitle_label.add_css_class("caption");
        cell.append(&title_label);
        cell.append(&subtitle_label);
        row.set_child(Some(&cell));
        ui.list.append(&row);
        rows.push(*id);
    }
    let current = state.borrow().current.as_ref().map(|note| note.meta.id);
    let target = current
        .and_then(|id| {
            rows.iter()
                .position(|row| *row == id)
                .map(|position| (position, true))
        })
        .or_else(|| (current.is_none() && !rows.is_empty()).then_some((0, false)));
    state.borrow_mut().rows = rows;
    if let Some((position, suppress)) = target
        && let Some(row) = ui.list.row_at_index(position as i32)
    {
        state.borrow_mut().suppress_select = suppress;
        ui.list.select_row(Some(&row));
        state.borrow_mut().suppress_select = false;
    }
}

/// The editor's text.
fn buffer_text(editor: &sourceview5::View) -> String {
    let buffer = editor.buffer();
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string()
}

/// Mark the buffer dirty and (re)arm the autosave debounce.
fn schedule_autosave(state: &Rc<RefCell<State>>, ui: &Ui) {
    let delay = state.borrow().autosave_ms;
    {
        let mut st = state.borrow_mut();
        st.dirty = true;
        if let Some(id) = st.autosave.take() {
            id.remove();
        }
    }
    set_status(state, ui, "Unsaved changes");
    let state_for_save = state.clone();
    let ui_for_save = ui.clone();
    let id = gtk::glib::timeout_add_local(Duration::from_millis(delay), move || {
        save_now(&state_for_save, &ui_for_save, true);
        gtk::glib::ControlFlow::Break
    });
    state.borrow_mut().autosave = Some(id);
}

/// Save the open note if it changed. Returns whether the file is up to date.
fn save_now(state: &Rc<RefCell<State>>, ui: &Ui, announce: bool) -> bool {
    let text = buffer_text(&ui.editor);
    let outcome = {
        let mut st = state.borrow_mut();
        if let Some(id) = st.autosave.take() {
            id.remove();
        }
        if !st.dirty {
            return true;
        }
        let State {
            workspace,
            current,
            dirty,
            ..
        } = &mut *st;
        match (workspace.as_mut(), current.as_mut()) {
            (Some(workspace), Some(note)) => {
                if note.body != text {
                    note.body = text;
                }
                match workspace.save_note(note) {
                    Ok(()) => {
                        *dirty = false;
                        Ok(())
                    }
                    Err(error) => {
                        *dirty = true;
                        Err(error.to_string())
                    }
                }
            }
            _ => Ok(()),
        }
    };
    match outcome {
        Ok(()) => {
            if announce {
                set_status(state, ui, "Saved");
            }
            true
        }
        Err(error) => {
            set_status(state, ui, "Save failed");
            ui.toast
                .add_toast(adw::Toast::new(&format!("Save failed: {error}")));
            false
        }
    }
}

fn load_note(state: &Rc<RefCell<State>>, ui: &Ui, id: NoteId) {
    save_now(state, ui, false);
    let loaded = {
        let st = state.borrow();
        st.workspace
            .as_ref()
            .and_then(|workspace| workspace.get_note(&id).ok().flatten())
    };
    let Some(note) = loaded else {
        return;
    };
    fill_editor(state, ui, note);
}

/// Push a note from disk into the editor without marking it dirty.
fn fill_editor(state: &Rc<RefCell<State>>, ui: &Ui, note: Note) {
    let title = note.meta.title.clone();
    let body = note.body.clone();
    state.borrow_mut().loading = true;
    ui.editor.buffer().set_text(&body);
    ui.preview.render(&body);
    {
        let mut st = state.borrow_mut();
        st.loading = false;
        st.dirty = false;
        st.current = Some(note);
    }
    ui.window.set_title(Some(&format!("{title} — Writan")));
    set_status(state, ui, "");
}

/// React to a settled batch of filesystem events, like the Tauri shell's
/// `vault-changed` handler: reindex the touched paths, refresh the list and
/// reload the open note unless it has unsaved edits.
fn handle_external(state: &Rc<RefCell<State>>, ui: &Ui, events: Vec<VaultEvent>) {
    let rels: Vec<String> = {
        let st = state.borrow();
        let Some(workspace) = st.workspace.as_ref() else {
            return;
        };
        events
            .iter()
            .filter_map(|event| {
                let path = match event {
                    VaultEvent::Upserted(path) | VaultEvent::Removed(path) => path,
                };
                workspace.vault().rel_path(path)
            })
            .collect()
    };
    if rels.is_empty() {
        return;
    }
    {
        let mut st = state.borrow_mut();
        if let Some(workspace) = st.workspace.as_mut() {
            let _ = workspace.reconcile_paths(&rels);
        }
    }
    refresh(state, ui);

    let changed_open_note = {
        let st = state.borrow();
        st.current
            .as_ref()
            .is_some_and(|note| rels.iter().any(|rel| rel == &note.rel_path))
    };
    if changed_open_note && !state.borrow().dirty {
        reload_current(state, ui);
    }
}

/// Reload the open note from disk when it is clean; without visible content
/// changes only its metadata is refreshed, so the cursor never jumps.
fn reload_current(state: &Rc<RefCell<State>>, ui: &Ui) {
    let loaded = {
        let st = state.borrow();
        let Some(id) = st.current.as_ref().map(|note| note.meta.id) else {
            return;
        };
        st.workspace
            .as_ref()
            .and_then(|workspace| workspace.get_note(&id).ok().flatten())
    };
    // The file may have been removed on disk; keep the buffer either way.
    let Some(note) = loaded else {
        return;
    };
    if buffer_text(&ui.editor) == note.body {
        state.borrow_mut().current = Some(note);
        return;
    }
    fill_editor(state, ui, note);
}

/// Show `text` in the header's status label; "Saved" clears itself.
fn set_status(state: &Rc<RefCell<State>>, ui: &Ui, text: &str) {
    ui.status.set_text(text);
    let mut st = state.borrow_mut();
    if let Some(id) = st.status_timer.take() {
        id.remove();
    }
    if text == "Saved" {
        let state = state.clone();
        let ui = ui.clone();
        let id = gtk::glib::timeout_add_seconds_local(2, move || {
            ui.status.set_text("");
            state.borrow_mut().status_timer = None;
            gtk::glib::ControlFlow::Break
        });
        st.status_timer = Some(id);
    }
}

fn read_vm_rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0)
}

fn load_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "scrolledwindow.preview-pane { background-color: alpha(currentColor, 0.04); }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Keep the editor's syntax scheme in step with the system preference.
///
/// GtkSourceView only picks its dark scheme when the GTK theme is literally
/// named `Adwaita-dark`; under GNOME's dark mode the name stays `Adwaita` and
/// the editor would render light inside a dark window.
fn apply_style_scheme(buffer: &sourceview5::Buffer) {
    let id = if adw::StyleManager::default().is_dark() {
        "Adwaita-dark"
    } else {
        "Adwaita"
    };
    if let Some(scheme) = sourceview5::StyleSchemeManager::new().scheme(id) {
        buffer.set_style_scheme(Some(&scheme));
    }
}
