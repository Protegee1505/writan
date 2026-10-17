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
    /// Active single-tag filter (`None` = every note).
    tag_filter: Option<String>,
    /// Active top-level folder filter.
    folder_filter: Option<String>,
    /// Two-step delete (`settings.general.confirmDelete`).
    confirm_delete: bool,
    delete_armed: bool,
    delete_timer: Option<gtk::glib::SourceId>,
    /// `settings.view.showWordCount`.
    show_word_count: bool,
    /// True while widgets are updated from code, so their signals do not
    /// re-enter the actions that set them.
    programmatic: bool,
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
    title_entry: gtk::Entry,
    words: gtk::Label,
    pin_button: gtk::ToggleButton,
    delete_button: gtk::Button,
    facets: gtk::Box,
    active_filters: gtk::Box,
    folders_flow: gtk::FlowBox,
    tags_flow: gtk::FlowBox,
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
        tag_filter: None,
        folder_filter: None,
        confirm_delete: config::load_confirm_delete(&config_path),
        delete_armed: false,
        delete_timer: None,
        show_word_count: config::load_show_word_count(&config_path),
        programmatic: false,
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

    // Facets, like the Tauri sidebar: active filters first, then folders and
    // the most used tags, all click-to-toggle.
    let active_filters = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    active_filters.set_halign(gtk::Align::Start);
    let folders_flow = facet_flow_box();
    let tags_flow = facet_flow_box();
    let facets = gtk::Box::new(gtk::Orientation::Vertical, 6);
    facets.set_margin_start(10);
    facets.set_margin_end(10);
    facets.set_margin_bottom(6);
    facets.append(&active_filters);
    facets.append(&facet_scroll(&folders_flow));
    facets.append(&facet_scroll(&tags_flow));

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.set_width_request(280);
    sidebar.append(&search);
    sidebar.append(&facets);
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

    // Note header above the editor, like the Tauri editor pane: title entry,
    // word count, pin and delete.
    let title_entry = gtk::Entry::builder()
        .placeholder_text("Untitled")
        .hexpand(true)
        .build();
    let words = gtk::Label::builder()
        .css_classes(["dim-label", "caption"])
        .build();
    let pin_button = gtk::ToggleButton::builder()
        .icon_name("view-pin-symbolic")
        .tooltip_text("Pin / unpin note")
        .css_classes(["flat"])
        .build();
    let delete_button = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Move to trash")
        .css_classes(["flat"])
        .build();
    let editor_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    editor_header.set_margin_start(18);
    editor_header.set_margin_end(18);
    editor_header.set_margin_top(10);
    editor_header.set_margin_bottom(4);
    editor_header.append(&title_entry);
    editor_header.append(&words);
    editor_header.append(&pin_button);
    editor_header.append(&delete_button);

    let editor_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    editor_pane.append(&editor_header);
    editor_pane.append(&editor_scroll);

    // Preview pane.
    let preview = Preview::new();
    let preview_scroll = gtk::ScrolledWindow::builder()
        .child(&preview.view)
        .hexpand(true)
        .vexpand(true)
        .build();
    preview_scroll.add_css_class("preview-pane");

    let inner = gtk::Paned::new(gtk::Orientation::Horizontal);
    inner.set_start_child(Some(&editor_pane));
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
        title_entry: title_entry.clone(),
        words: words.clone(),
        pin_button: pin_button.clone(),
        delete_button: delete_button.clone(),
        facets: facets.clone(),
        active_filters: active_filters.clone(),
        folders_flow: folders_flow.clone(),
        tags_flow: tags_flow.clone(),
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
        search.connect_search_changed(move |_| refresh(&state, &ui, false));
    }

    // Rename: commit the title entry on Enter or when focus leaves it.
    {
        let state_for_enter = state.clone();
        let ui_for_enter = ui.clone();
        title_entry.connect_activate(move |_| commit_title(&state_for_enter, &ui_for_enter));
        let state_for_focus = state.clone();
        let ui_for_focus = ui.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |_| commit_title(&state_for_focus, &ui_for_focus));
        title_entry.add_controller(focus);
    }

    // Pin / unpin.
    {
        let state = state.clone();
        let ui = ui.clone();
        pin_button.connect_toggled(move |_| pin_toggled(&state, &ui));
    }

    // Move to trash, with the Tauri shell's two-step confirm.
    {
        let state = state.clone();
        let ui = ui.clone();
        delete_button.connect_clicked(move |_| delete_clicked(&state, &ui));
    }

    // Editor → preview, dirty state, debounced autosave.
    {
        let state = state.clone();
        let ui = ui.clone();
        buffer.connect_changed(move |buffer| {
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            preview.render(&text);
            update_words(&state, &ui, &text);
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
            refresh(&state, &ui, false);
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
        refresh(&state, &ui, true);
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
                refresh(&state, &ui, true);
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

/// Rebuild the note list for the current search and filters. `fallback` opens
/// the first row when nothing is open — only for user-driven entry points
/// (startup, vault switch), so deletes and external changes never yank the
/// editor to another note.
fn refresh(state: &Rc<RefCell<State>>, ui: &Ui, fallback: bool) {
    while let Some(child) = ui.list.first_child() {
        ui.list.remove(&child);
    }
    let text = ui.search.text().trim().to_string();
    let (tag, folder) = {
        let st = state.borrow();
        (st.tag_filter.clone(), st.folder_filter.clone())
    };
    let filtering = !text.is_empty() || tag.is_some() || folder.is_some();
    let entries: Vec<(NoteId, String, String, bool)> = {
        let st = state.borrow();
        let Some(workspace) = st.workspace.as_ref() else {
            return;
        };
        if filtering {
            workspace
                .search(&SearchQuery {
                    text,
                    tags: tag.into_iter().collect(),
                    folder,
                    limit: Some(500),
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
                    (hit.id, hit.title, subtitle, hit.pinned)
                })
                .collect()
        } else {
            workspace
                .list(500)
                .unwrap_or_default()
                .into_iter()
                .map(|note| (note.id, note.title, note.rel_path, note.pinned))
                .collect()
        }
    };
    let mut rows = Vec::with_capacity(entries.len());
    for (id, title, subtitle, pinned) in &entries {
        let row = gtk::ListBoxRow::new();
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        cell.set_margin_start(10);
        cell.set_margin_end(10);
        cell.set_margin_top(6);
        cell.set_margin_bottom(6);
        let lines = gtk::Box::new(gtk::Orientation::Vertical, 2);
        lines.set_hexpand(true);
        let title_label = gtk::Label::new(Some(title));
        title_label.set_xalign(0.0);
        title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let subtitle_label = gtk::Label::new(Some(subtitle));
        subtitle_label.set_xalign(0.0);
        subtitle_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle_label.add_css_class("dim-label");
        subtitle_label.add_css_class("caption");
        lines.append(&title_label);
        lines.append(&subtitle_label);
        cell.append(&lines);
        if *pinned {
            let pin = gtk::Label::builder()
                .label("●")
                .css_classes(["dim-label", "caption"])
                .tooltip_text("Pinned")
                .build();
            cell.append(&pin);
        }
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
        .or_else(|| (fallback && current.is_none() && !rows.is_empty()).then_some((0, false)));
    state.borrow_mut().rows = rows;
    if let Some((position, suppress)) = target
        && let Some(row) = ui.list.row_at_index(position as i32)
    {
        state.borrow_mut().suppress_select = suppress;
        ui.list.select_row(Some(&row));
        state.borrow_mut().suppress_select = false;
    }
    refresh_facets(state, ui);
}

/// Rebuild the sidebar facets from the index, mirroring the Tauri sidebar's
/// chips: active filters with their × and a Clear, then folders and the most
/// used tags, all click-to-toggle.
fn refresh_facets(state: &Rc<RefCell<State>>, ui: &Ui) {
    while let Some(child) = ui.folders_flow.first_child() {
        ui.folders_flow.remove(&child);
    }
    while let Some(child) = ui.tags_flow.first_child() {
        ui.tags_flow.remove(&child);
    }
    while let Some(child) = ui.active_filters.first_child() {
        ui.active_filters.remove(&child);
    }
    let (folders, tags, tag_filter, folder_filter) = {
        let st = state.borrow();
        let Some(workspace) = st.workspace.as_ref() else {
            ui.facets.set_visible(false);
            return;
        };
        (
            workspace.folder_counts().unwrap_or_default(),
            workspace.tag_counts().unwrap_or_default(),
            st.tag_filter.clone(),
            st.folder_filter.clone(),
        )
    };

    {
        let mut st = state.borrow_mut();
        st.programmatic = true;
    }
    if let Some(folder) = &folder_filter {
        let chip = facet_clear_chip(&format!("{folder} ×"), "Clear folder filter");
        let state_for_chip = state.clone();
        let ui_for_chip = ui.clone();
        chip.connect_clicked(move |_| {
            state_for_chip.borrow_mut().folder_filter = None;
            refresh(&state_for_chip, &ui_for_chip, false);
        });
        ui.active_filters.append(&chip);
    }
    if let Some(tag) = &tag_filter {
        let chip = facet_clear_chip(&format!("#{tag} ×"), "Clear tag filter");
        let state_for_chip = state.clone();
        let ui_for_chip = ui.clone();
        chip.connect_clicked(move |_| {
            state_for_chip.borrow_mut().tag_filter = None;
            refresh(&state_for_chip, &ui_for_chip, false);
        });
        ui.active_filters.append(&chip);
    }
    if tag_filter.is_some() || folder_filter.is_some() {
        let clear = gtk::Button::builder()
            .label("Clear")
            .tooltip_text("Clear filters")
            .css_classes(["flat", "caption"])
            .build();
        let state_for_chip = state.clone();
        let ui_for_chip = ui.clone();
        clear.connect_clicked(move |_| {
            {
                let mut st = state_for_chip.borrow_mut();
                st.tag_filter = None;
                st.folder_filter = None;
            }
            refresh(&state_for_chip, &ui_for_chip, false);
        });
        ui.active_filters.append(&clear);
    }

    for (name, count) in &folders {
        let chip = gtk::ToggleButton::builder()
            .label(format!("{name} {count}"))
            .tooltip_text(name)
            .css_classes(["pill", "caption"])
            .active(folder_filter.as_deref() == Some(name.as_str()))
            .build();
        let state_for_chip = state.clone();
        let ui_for_chip = ui.clone();
        let name = name.clone();
        chip.connect_toggled(move |button| {
            if state_for_chip.borrow().programmatic {
                return;
            }
            state_for_chip.borrow_mut().folder_filter = button.is_active().then(|| name.clone());
            refresh(&state_for_chip, &ui_for_chip, false);
        });
        ui.folders_flow.append(&chip);
    }
    for (name, count) in tags.iter().take(18) {
        let chip = gtk::ToggleButton::builder()
            .label(format!("#{name} {count}"))
            .tooltip_text(format!("#{name}"))
            .css_classes(["pill", "caption"])
            .active(tag_filter.as_deref() == Some(name.as_str()))
            .build();
        let state_for_chip = state.clone();
        let ui_for_chip = ui.clone();
        let name = name.clone();
        chip.connect_toggled(move |button| {
            if state_for_chip.borrow().programmatic {
                return;
            }
            state_for_chip.borrow_mut().tag_filter = button.is_active().then(|| name.clone());
            refresh(&state_for_chip, &ui_for_chip, false);
        });
        ui.tags_flow.append(&chip);
    }
    state.borrow_mut().programmatic = false;

    ui.active_filters
        .set_visible(tag_filter.is_some() || folder_filter.is_some());
    ui.facets.set_visible(
        !folders.is_empty() || !tags.is_empty() || tag_filter.is_some() || folder_filter.is_some(),
    );
}

/// A small chip that clears the filter it names.
fn facet_clear_chip(label: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .tooltip_text(tooltip)
        .css_classes(["pill", "caption"])
        .build()
}

/// The wrapping container for one facet section.
fn facet_flow_box() -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(6)
        .row_spacing(6)
        .build()
}

/// A FlowBox is only measured with height-for-width when a ScrolledWindow
/// drives it; inside a plain box it gets a single row and its other rows draw
/// over the widgets below. `propagate_natural_height` keeps it as tall as its
/// content (up to the cap), the scrollbar appears beyond that.
fn facet_scroll(flow: &gtk::FlowBox) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(flow)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .max_content_height(190)
        .build()
}

/// Rename the open note from the title entry, like the Tauri shell's title
/// input: trimmed, empty titles are refused, focus leaving commits.
fn commit_title(state: &Rc<RefCell<State>>, ui: &Ui) {
    let text = ui.title_entry.text().trim().to_string();
    let open = state
        .borrow()
        .current
        .as_ref()
        .map(|note| (note.meta.id, note.meta.title.clone()));
    let Some((id, title)) = open else {
        return;
    };
    if text.is_empty() {
        ui.title_entry.set_text(&title);
        return;
    }
    if text == title {
        return;
    }
    save_now(state, ui, false);
    let outcome = {
        let mut st = state.borrow_mut();
        let State {
            workspace, current, ..
        } = &mut *st;
        match (workspace.as_mut(), current.as_mut()) {
            (Some(workspace), Some(note)) if note.meta.id == id => workspace
                .rename_note(note, &text)
                .map_err(|e| e.to_string()),
            _ => Ok(()),
        }
    };
    match outcome {
        Ok(()) => {
            let renamed = state
                .borrow()
                .current
                .as_ref()
                .map(|note| note.meta.title.clone());
            let title = renamed.unwrap_or(text);
            ui.window.set_title(Some(&format!("{title} — Writan")));
            refresh(state, ui, false);
        }
        Err(error) => {
            ui.title_entry.set_text(&title);
            ui.toast
                .add_toast(adw::Toast::new(&format!("Rename failed: {error}")));
        }
    }
}

/// Pin / unpin the open note; pinned notes sort first, like the Tauri shell.
fn pin_toggled(state: &Rc<RefCell<State>>, ui: &Ui) {
    if state.borrow().programmatic {
        return;
    }
    let pinned = ui.pin_button.is_active();
    let id = state.borrow().current.as_ref().map(|note| note.meta.id);
    let Some(id) = id else {
        ui.pin_button.set_active(false);
        return;
    };
    save_now(state, ui, false);
    let outcome = {
        let mut st = state.borrow_mut();
        match st.workspace.as_mut() {
            Some(workspace) => workspace
                .set_pinned(&id, pinned)
                .map(Some)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    };
    match outcome {
        Ok(Some(note)) => {
            if let Some(current) = state.borrow_mut().current.as_mut() {
                current.meta.pinned = note.meta.pinned;
            }
            refresh(state, ui, false);
        }
        Ok(None) => {}
        Err(error) => {
            {
                let mut st = state.borrow_mut();
                st.programmatic = true;
                ui.pin_button.set_active(!pinned);
                st.programmatic = false;
            }
            ui.toast
                .add_toast(adw::Toast::new(&format!("Pin failed: {error}")));
        }
    }
}

/// First click arms the delete button, a second click within three seconds
/// moves the note to the trash (`settings.general.confirmDelete`).
fn delete_clicked(state: &Rc<RefCell<State>>, ui: &Ui) {
    if state.borrow().current.is_none() {
        return;
    }
    let (confirm, armed) = {
        let st = state.borrow();
        (st.confirm_delete, st.delete_armed)
    };
    if !confirm || armed {
        move_to_trash(state, ui);
        return;
    }
    state.borrow_mut().delete_armed = true;
    ui.delete_button.set_icon_name("user-trash-full-symbolic");
    ui.delete_button
        .set_tooltip_text(Some("Click again to move to trash"));
    ui.toast
        .add_toast(adw::Toast::new("Click the trash button again to delete"));
    let state_for_timer = state.clone();
    let ui_for_timer = ui.clone();
    let id = gtk::glib::timeout_add_seconds_local(3, move || {
        disarm_delete(&state_for_timer, &ui_for_timer);
        gtk::glib::ControlFlow::Break
    });
    state.borrow_mut().delete_timer = Some(id);
}

fn disarm_delete(state: &Rc<RefCell<State>>, ui: &Ui) {
    {
        let mut st = state.borrow_mut();
        if let Some(timer) = st.delete_timer.take() {
            timer.remove();
        }
        st.delete_armed = false;
    }
    ui.delete_button.set_icon_name("user-trash-symbolic");
    ui.delete_button.set_tooltip_text(Some("Move to trash"));
}

/// Move the open note to the trash and clear the editor (the Tauri shell
/// leaves nothing selected).
fn move_to_trash(state: &Rc<RefCell<State>>, ui: &Ui) {
    let id = state.borrow().current.as_ref().map(|note| note.meta.id);
    let Some(id) = id else {
        return;
    };
    let outcome = {
        let mut st = state.borrow_mut();
        match st.workspace.as_mut() {
            Some(workspace) => workspace
                .delete_note(&id)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            None => return,
        }
    };
    match outcome {
        Ok(()) => {
            {
                let mut st = state.borrow_mut();
                if let Some(save) = st.autosave.take() {
                    save.remove();
                }
                st.current = None;
                st.dirty = false;
                st.loading = true;
            }
            ui.editor.buffer().set_text("");
            state.borrow_mut().loading = false;
            ui.preview.render("");
            ui.title_entry.set_text("");
            ui.words.set_text("");
            disarm_delete(state, ui);
            ui.window.set_title(Some("Writan"));
            set_status(state, ui, "");
            refresh(state, ui, false);
        }
        Err(error) => {
            ui.toast
                .add_toast(adw::Toast::new(&format!("Delete failed: {error}")));
        }
    }
}

/// `settings.view.showWordCount` — the Tauri editor header shows this.
fn update_words(state: &Rc<RefCell<State>>, ui: &Ui, body: &str) {
    if !state.borrow().show_word_count {
        ui.words.set_text("");
        return;
    }
    ui.words
        .set_text(&format!("{} words", body.split_whitespace().count()));
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
    let pinned = note.meta.pinned;
    {
        let mut st = state.borrow_mut();
        st.loading = true;
        st.programmatic = true;
    }
    ui.editor.buffer().set_text(&body);
    ui.preview.render(&body);
    ui.title_entry.set_text(&title);
    ui.pin_button.set_active(pinned);
    update_words(state, ui, &body);
    disarm_delete(state, ui);
    {
        let mut st = state.borrow_mut();
        st.loading = false;
        st.programmatic = false;
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
    refresh(state, ui, false);

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
