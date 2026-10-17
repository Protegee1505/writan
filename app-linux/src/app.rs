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

use writan_core::{Note, NoteId, SearchQuery, TrashEntry, VaultEvent, VaultWatcher, Workspace};

use crate::config;
use crate::keymap;
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
    /// `settings.view.dateFormat` for trash timestamps.
    date_format: crate::dates::DateFormat,
    /// True while widgets are updated from code, so their signals do not
    /// re-enter the actions that set them.
    programmatic: bool,
    /// Shortcut overrides from `settings.shortcuts` (keymap.rs).
    overrides: keymap::ShortcutOverrides,
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
    let overrides = keymap::load_overrides(&config_path);
    warn_reserved_overrides(&overrides);
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
        date_format: config::load_date_format(&config_path),
        programmatic: false,
        overrides: overrides.clone(),
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
        .tooltip_text(keyed_tooltip("new-note", &overrides))
        .build();
    let preview_toggle = gtk::ToggleButton::builder()
        .icon_name("view-reveal-symbolic")
        .tooltip_text("Toggle preview")
        .active(true)
        .build();
    let trash_button = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Trash")
        .build();
    let settings_button = gtk::Button::builder()
        .icon_name("emblem-system-symbolic")
        .tooltip_text(keyed_tooltip("open-settings", &overrides))
        .build();
    header.pack_start(&open_button);
    header.pack_start(&status);
    header.pack_end(&preview_toggle);
    header.pack_end(&trash_button);
    header.pack_end(&settings_button);
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

    // Open the trash browser.
    {
        let state = state.clone();
        let ui = ui.clone();
        trash_button.connect_clicked(move |_| open_trash_or_focus(&state, &ui));
    }

    // Open the settings panel.
    {
        let state = state.clone();
        let ui = ui.clone();
        settings_button.connect_clicked(move |_| open_settings_or_focus(&state, &ui));
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
        let create = move || create_note(&state, &ui);
        let create2 = create.clone();
        new_button.connect_clicked(move |_| create());
        let new_action = gtk::gio::SimpleAction::new("new-note", None);
        new_action.connect_activate(move |_, _| create2());
        application.add_action(&new_action);
        apply_accels(application, &overrides, "new-note");
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

    // Save the open note.
    let save_action = gtk::gio::SimpleAction::new("save-note", None);
    {
        let state = state.clone();
        let ui = ui.clone();
        save_action.connect_activate(move |_, _| {
            save_now(&state, &ui, true);
        });
    }
    application.add_action(&save_action);
    apply_accels(application, &overrides, "save-note");

    // Focus the sidebar search.
    let find_action = gtk::gio::SimpleAction::new("focus-search", None);
    {
        let search = search.clone();
        find_action.connect_activate(move |_, _| {
            search.grab_focus();
        });
    }
    application.add_action(&find_action);
    apply_accels(application, &overrides, "focus-search");

    // Command palette (Ctrl+P, alias Ctrl+K).
    {
        let state = state.clone();
        let ui = ui.clone();
        let palette_action = gtk::gio::SimpleAction::new("command-palette", None);
        palette_action.connect_activate(move |_, _| open_palette(&state, &ui));
        application.add_action(&palette_action);
        apply_accels(application, &overrides, "command-palette");
    }

    // Trash panel and pin toggle, so `settings.shortcuts` can bind them
    // (keymap.ts leaves both unbound by default).
    {
        let state = state.clone();
        let ui = ui.clone();
        let trash_action = gtk::gio::SimpleAction::new("toggle-trash", None);
        trash_action.connect_activate(move |_, _| open_trash_or_focus(&state, &ui));
        application.add_action(&trash_action);
        apply_accels(application, &overrides, "toggle-trash");
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let pin_action = gtk::gio::SimpleAction::new("pin-note", None);
        pin_action.connect_activate(move |_, _| toggle_pin(&state, &ui));
        application.add_action(&pin_action);
        apply_accels(application, &overrides, "pin-note");
    }

    // Settings (Ctrl+,): the header gear and the palette's "Open settings…".
    {
        let state = state.clone();
        let ui = ui.clone();
        let settings_action = gtk::gio::SimpleAction::new("open-settings", None);
        settings_action.connect_activate(move |_, _| open_settings_or_focus(&state, &ui));
        application.add_action(&settings_action);
        apply_accels(application, &overrides, "open-settings");
    }

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

/// Install an action's keymap accelerators (the effective binding plus the
/// always-on aliases) on the application.
fn apply_accels<A: IsA<gtk::Application>>(
    application: &A,
    overrides: &keymap::ShortcutOverrides,
    id: &str,
) {
    let accels = keymap::accels_for(id, overrides);
    let accels: Vec<&str> = accels.iter().map(String::as_str).collect();
    application.set_accels_for_action(&format!("app.{id}"), &accels);
}

/// `"Settings (Ctrl+,)"` — an action's label and its current binding, the
/// header-button tooltip shape.
fn keyed_tooltip(id: &str, overrides: &keymap::ShortcutOverrides) -> String {
    let hint = keymap::hint(id, overrides);
    if hint.is_empty() {
        keymap::label(id).to_string()
    } else {
        format!("{} ({hint})", keymap::label(id))
    }
}

/// Reinstall every action's accelerators (used after "Reset all settings").
fn apply_all_accels(application: &gtk::Application, overrides: &keymap::ShortcutOverrides) {
    for action in keymap::KEY_ACTIONS {
        apply_accels(application, overrides, action.id);
    }
}

/// Present the application's window titled `title`; true when one was found.
fn focus_window(application: &gtk::Application, title: &str) -> bool {
    for window in application.windows() {
        if window.title().as_deref() == Some(title) {
            window.present();
            return true;
        }
    }
    false
}

/// A hand-edited config must not hijack text-editing keys: `keymap.rs`
/// refuses reserved overrides, and we say so once per bad entry.
fn warn_reserved_overrides(overrides: &keymap::ShortcutOverrides) {
    for action in keymap::KEY_ACTIONS {
        if let Some(Some(binding)) = overrides.get(action.id)
            && keymap::is_reserved(binding)
        {
            eprintln!(
                "writan: shortcuts.{} = {binding:?} is reserved; keeping {}",
                action.id,
                action.default_binding.unwrap_or("unbound")
            );
        }
    }
}

/// Create a note and open it (the Tauri shell's New note action, and the
/// palette's first item).
fn create_note(state: &Rc<RefCell<State>>, ui: &Ui) {
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
    refresh(state, ui, false);
    open_note_by_id(state, ui, note.meta.id);
    ui.editor.grab_focus();
}

/// Select a note's row (which loads it) or, when the current search or
/// filters hide it, load it directly.
fn open_note_by_id(state: &Rc<RefCell<State>>, ui: &Ui, id: NoteId) {
    let position = state.borrow().rows.iter().position(|row| *row == id);
    match position.and_then(|position| ui.list.row_at_index(position as i32)) {
        Some(row) => {
            ui.list.select_row(Some(&row));
        }
        None => load_note(state, ui, id),
    }
}

/// Flip the pin of the open note; the button's own `toggled` handler writes
/// it, so the palette goes through the same path as the header button.
fn toggle_pin(state: &Rc<RefCell<State>>, ui: &Ui) {
    if state.borrow().current.is_some() {
        let active = ui.pin_button.is_active();
        ui.pin_button.set_active(!active);
    }
}

/// Open the trash panel, or raise the one already up (application
/// accelerators can fire while the modal panel holds focus).
fn open_trash_or_focus(state: &Rc<RefCell<State>>, ui: &Ui) {
    if let Some(application) = ui.window.application()
        && focus_window(&application, "Trash")
    {
        return;
    }
    open_trash(state, ui);
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

/// The notes the sidebar shows for the current search and filters, in list
/// order: (id, title, subtitle, pinned). The command palette's note items
/// come from here too, so both agree on what "visible" means.
fn visible_entries(state: &Rc<RefCell<State>>, text: &str) -> Vec<(NoteId, String, String, bool)> {
    let text = text.trim();
    let (tag, folder) = {
        let st = state.borrow();
        (st.tag_filter.clone(), st.folder_filter.clone())
    };
    let filtering = !text.is_empty() || tag.is_some() || folder.is_some();
    let st = state.borrow();
    let Some(workspace) = st.workspace.as_ref() else {
        return Vec::new();
    };
    if filtering {
        workspace
            .search(&SearchQuery {
                text: text.to_string(),
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
}

/// Rebuild the note list for the current search and filters. `fallback` opens
/// the first row when nothing is open — only for user-driven entry points
/// (startup, vault switch), so deletes and external changes never yank the
/// editor to another note.
fn refresh(state: &Rc<RefCell<State>>, ui: &Ui, fallback: bool) {
    while let Some(child) = ui.list.first_child() {
        ui.list.remove(&child);
    }
    let entries = visible_entries(state, &ui.search.text());
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

/// The trash browser: notes `move_to_trash` moved out of the vault, each
/// restorable or purgeable. A fresh modal `adw::Window` per open, like the
/// Tauri shell's modal panel; while it is up the editor cannot change.
fn open_trash(state: &Rc<RefCell<State>>, ui: &Ui) {
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rows.set_margin_top(6);
    rows.set_margin_bottom(6);
    rows.set_margin_start(6);
    rows.set_margin_end(6);

    let empty = gtk::Label::builder()
        .label("Trash is empty.")
        .css_classes(["dim-label"])
        .margin_top(48)
        .margin_bottom(48)
        .build();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&rows);
    body.append(&empty);

    let scroll = gtk::ScrolledWindow::builder()
        .child(&body)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();

    let header = adw::HeaderBar::new();
    let empty_button = gtk::Button::builder()
        .label("Empty Trash")
        .tooltip_text("Delete every trashed note forever")
        .build();
    header.pack_end(&empty_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scroll));

    let window = adw::Window::builder()
        .title("Trash")
        .transient_for(&ui.window)
        .modal(true)
        .default_width(640)
        .default_height(520)
        .content(&toolbar)
        .build();

    let panel = TrashPanel {
        rows: rows.clone(),
        empty: empty.clone(),
        empty_button: empty_button.clone(),
    };
    fill_trash(state, ui, &panel);

    {
        let state = state.clone();
        let ui = ui.clone();
        let panel = panel.clone();
        empty_button.connect_clicked(move |_| {
            let outcome = {
                let st = state.borrow();
                let Some(workspace) = st.workspace.as_ref() else {
                    return;
                };
                workspace.trash().empty().map_err(|error| error.to_string())
            };
            match outcome {
                Ok(count) => {
                    if count > 0 {
                        ui.toast.add_toast(adw::Toast::new(&format!(
                            "Removed {count} note{}",
                            if count == 1 { "" } else { "s" }
                        )));
                    }
                    fill_trash(&state, &ui, &panel);
                }
                Err(error) => {
                    ui.toast
                        .add_toast(adw::Toast::new(&format!("Empty trash failed: {error}")));
                }
            }
        });
    }

    // Escape closes the panel, like every GNOME dialog.
    {
        let window_for_escape = window.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                window_for_escape.close();
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        window.add_controller(keys);
    }

    window.present();
}

/// Widgets of the open trash window; rows are rebuilt in place after every
/// restore, purge or empty.
#[derive(Clone)]
struct TrashPanel {
    rows: gtk::Box,
    empty: gtk::Label,
    empty_button: gtk::Button,
}

/// Rebuild the trash list from `Trash::list`.
fn fill_trash(state: &Rc<RefCell<State>>, ui: &Ui, panel: &TrashPanel) {
    while let Some(child) = panel.rows.first_child() {
        panel.rows.remove(&child);
    }
    let entries = {
        let st = state.borrow();
        let Some(workspace) = st.workspace.as_ref() else {
            return;
        };
        workspace.trash().list().unwrap_or_default()
    };
    panel.empty.set_visible(entries.is_empty());
    panel.empty_button.set_sensitive(!entries.is_empty());
    for entry in &entries {
        panel.rows.append(&trash_row(state, ui, panel, entry));
    }
}

/// One trashed note: title and deletion time, with Restore and Delete forever.
fn trash_row(
    state: &Rc<RefCell<State>>,
    ui: &Ui,
    panel: &TrashPanel,
    entry: &TrashEntry,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(12);
    row.set_margin_end(12);

    let info = gtk::Box::new(gtk::Orientation::Vertical, 2);
    info.set_hexpand(true);
    let title = gtk::Label::builder()
        .label(if entry.title.is_empty() {
            "Untitled"
        } else {
            &entry.title
        })
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let deleted = {
        let st = state.borrow();
        crate::dates::fmt_date(&entry.deleted_at, st.date_format)
    };
    let meta = gtk::Label::builder()
        .label(format!("{deleted} · {}", entry.original_path))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label", "caption"])
        .build();
    info.append(&title);
    info.append(&meta);
    row.append(&info);

    let restore = gtk::Button::builder()
        .label("Restore")
        .tooltip_text("Move the note back into the vault")
        .valign(gtk::Align::Center)
        .build();
    let purge = gtk::Button::builder()
        .label("Delete forever")
        .tooltip_text("Remove the note from the trash for good")
        .css_classes(["destructive-action"])
        .valign(gtk::Align::Center)
        .build();
    row.append(&restore);
    row.append(&purge);

    let id = entry.id;
    {
        let state = state.clone();
        let ui = ui.clone();
        let panel = panel.clone();
        restore.connect_clicked(move |_| restore_trash_entry(&state, &ui, &panel, &id));
    }
    {
        let state = state.clone();
        let ui = ui.clone();
        let panel = panel.clone();
        purge.connect_clicked(move |_| purge_trash_entry(&state, &ui, &panel, &id));
    }
    row
}

/// Put a trashed note back into the vault (Tauri: `restore_note`).
fn restore_trash_entry(state: &Rc<RefCell<State>>, ui: &Ui, panel: &TrashPanel, id: &NoteId) {
    let outcome = {
        let mut st = state.borrow_mut();
        let Some(workspace) = st.workspace.as_mut() else {
            return;
        };
        workspace
            .restore_note(id)
            .map(|_| ())
            .map_err(|error| error.to_string())
    };
    match outcome {
        Ok(()) => {
            refresh(state, ui, false);
            fill_trash(state, ui, panel);
        }
        Err(error) => {
            ui.toast
                .add_toast(adw::Toast::new(&format!("Restore failed: {error}")));
        }
    }
}

/// Delete a trashed note for good (Tauri: `purge_note`).
fn purge_trash_entry(state: &Rc<RefCell<State>>, ui: &Ui, panel: &TrashPanel, id: &NoteId) {
    let outcome = {
        let st = state.borrow();
        let Some(workspace) = st.workspace.as_ref() else {
            return;
        };
        workspace
            .trash()
            .purge(id)
            .map_err(|error| error.to_string())
    };
    match outcome {
        Ok(()) => fill_trash(state, ui, panel),
        Err(error) => {
            ui.toast
                .add_toast(adw::Toast::new(&format!("Delete failed: {error}")));
        }
    }
}

/// The command palette: one search entry over the same actions as the Tauri
/// palette (`components/CommandPalette.svelte`) plus the notes the sidebar
/// currently shows. A modal window, like the trash panel.
fn open_palette(state: &Rc<RefCell<State>>, ui: &Ui) {
    if let Some(application) = ui.window.application()
        && focus_window(&application, "Command palette")
    {
        return;
    }

    let entry = gtk::Entry::builder()
        .placeholder_text("Type a command or note name…")
        .margin_start(10)
        .margin_end(10)
        .margin_top(10)
        .margin_bottom(6)
        .build();

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("navigation-sidebar");

    let scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();

    let empty = gtk::Label::builder()
        .label("No matches")
        .css_classes(["dim-label"])
        .margin_top(32)
        .margin_bottom(32)
        .build();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&entry);
    body.append(&empty);
    body.append(&scroll);
    body.set_margin_start(6);
    body.set_margin_end(6);
    body.set_margin_bottom(6);

    let window = adw::Window::builder()
        .title("Command palette")
        .transient_for(&ui.window)
        .modal(true)
        .default_width(520)
        .default_height(420)
        .content(&body)
        .build();

    let palette = Palette {
        entry: entry.clone(),
        list: list.clone(),
        empty: empty.clone(),
        commands: Rc::new(RefCell::new(Vec::new())),
    };
    fill_palette(state, ui, &palette);

    // Filter as the query changes; the selection returns to the first row.
    {
        let state = state.clone();
        let ui = ui.clone();
        let palette = palette.clone();
        entry.connect_changed(move |_| fill_palette(&state, &ui, &palette));
    }

    // Enter runs the selected item, then the window closes (Tauri's
    // `runItem`: close, then run).
    {
        let state = state.clone();
        let ui = ui.clone();
        let palette = palette.clone();
        let window_for_enter = window.clone();
        entry.connect_activate(move |_| {
            let index = palette
                .list
                .selected_row()
                .map(|row| row.index())
                .unwrap_or(0);
            let command = item_command(&palette, index);
            window_for_enter.close();
            if let Some(command) = command {
                run_palette_command(&state, &ui, command);
            }
        });
    }

    // A click on a row does the same.
    {
        let state = state.clone();
        let ui = ui.clone();
        let palette = palette.clone();
        let window_for_click = window.clone();
        list.connect_row_activated(move |_, row| {
            let command = item_command(&palette, row.index());
            window_for_click.close();
            if let Some(command) = command {
                run_palette_command(&state, &ui, command);
            }
        });
    }

    // Up/Down move the selection while the entry keeps focus.
    {
        let palette = palette.clone();
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gtk::gdk::Key::Down => {
                move_palette_selection(&palette, 1);
                gtk::glib::Propagation::Stop
            }
            gtk::gdk::Key::Up => {
                move_palette_selection(&palette, -1);
                gtk::glib::Propagation::Stop
            }
            _ => gtk::glib::Propagation::Proceed,
        });
        entry.add_controller(keys);
    }

    // Escape closes the palette, like every GNOME dialog.
    {
        let window_for_escape = window.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                window_for_escape.close();
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        window.add_controller(keys);
    }

    window.present();
    entry.grab_focus();
}

/// The palette's items, in the Tauri palette's order: actions first, then
/// the notes the sidebar shows, each with its key hint.
fn palette_items(state: &Rc<RefCell<State>>, ui: &Ui) -> Vec<PaletteItem> {
    let st = state.borrow();
    let overrides = &st.overrides;
    let mut items = Vec::new();
    let settings_item = || PaletteItem {
        label: "Open settings…".to_string(),
        hint: keymap::hint("open-settings", overrides),
        command: PaletteCommand::OpenSettings,
    };
    if st.workspace.is_some() {
        items.push(PaletteItem {
            label: keymap::label("new-note").to_string(),
            hint: keymap::hint("new-note", overrides),
            command: PaletteCommand::NewNote,
        });
        items.push(PaletteItem {
            label: "Save now".to_string(),
            hint: keymap::hint("save-note", overrides),
            command: PaletteCommand::SaveNow,
        });
        items.push(settings_item());
        items.push(PaletteItem {
            label: "Open trash".to_string(),
            hint: "Trash".to_string(),
            command: PaletteCommand::OpenTrash,
        });
        items.push(PaletteItem {
            label: "Rebuild search index".to_string(),
            hint: "Maintenance".to_string(),
            command: PaletteCommand::RebuildIndex,
        });
        if let Some(note) = st.current.as_ref() {
            items.push(PaletteItem {
                label: if note.meta.pinned {
                    "Unpin this note".to_string()
                } else {
                    "Pin this note".to_string()
                },
                hint: "Pin".to_string(),
                command: PaletteCommand::TogglePin,
            });
        }
    } else {
        // The Tauri palette has no such item: its welcome page is the only
        // way in. The GTK shell keeps the sidebar usable without a vault, so
        // the palette offers the folder picker instead.
        items.push(PaletteItem {
            label: "Choose notes folder…".to_string(),
            hint: String::new(),
            command: PaletteCommand::ChooseFolder,
        });
        items.push(settings_item());
    }
    for (id, title, _, _) in visible_entries(state, &ui.search.text()) {
        items.push(PaletteItem {
            label: if title.is_empty() {
                "Untitled".to_string()
            } else {
                title
            },
            hint: "Open note".to_string(),
            command: PaletteCommand::OpenNote(id),
        });
    }
    items
}

/// Rebuild the palette's rows for the current query and select the first.
fn fill_palette(state: &Rc<RefCell<State>>, ui: &Ui, palette: &Palette) {
    while let Some(child) = palette.list.first_child() {
        palette.list.remove(&child);
    }
    let query = palette.entry.text().trim().to_lowercase();
    let items: Vec<PaletteItem> = palette_items(state, ui)
        .into_iter()
        .filter(|item| query.is_empty() || item.label.to_lowercase().contains(&query))
        .collect();
    let no_items = items.is_empty();
    for item in &items {
        palette.list.append(&palette_row(item));
    }
    palette.empty.set_visible(no_items);
    if let Some(row) = palette.list.row_at_index(0) {
        palette.list.select_row(Some(&row));
    }
    palette.commands.replace(items);
}

/// One palette row: label left, key hint (or band) right.
fn palette_row(item: &PaletteItem) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let cell = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    cell.set_margin_start(10);
    cell.set_margin_end(10);
    cell.set_margin_top(8);
    cell.set_margin_bottom(8);
    let label = gtk::Label::builder()
        .label(&item.label)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    cell.append(&label);
    if !item.hint.is_empty() {
        let hint = gtk::Label::builder()
            .label(&item.hint)
            .css_classes(["dim-label", "caption"])
            .build();
        cell.append(&hint);
    }
    row.set_child(Some(&cell));
    row
}

/// The command of the palette row at `index`, if any.
fn item_command(palette: &Palette, index: i32) -> Option<PaletteCommand> {
    if index < 0 {
        return None;
    }
    palette
        .commands
        .borrow()
        .get(index as usize)
        .map(|item| item.command.clone())
}

/// Move the palette selection by `delta`, clamped to the rows on screen.
fn move_palette_selection(palette: &Palette, delta: i32) {
    let count = palette.commands.borrow().len() as i32;
    if count == 0 {
        return;
    }
    let current = palette
        .list
        .selected_row()
        .map(|row| row.index())
        .unwrap_or(0);
    let next = (current + delta).clamp(0, count - 1);
    if let Some(row) = palette.list.row_at_index(next) {
        palette.list.select_row(Some(&row));
    }
}

/// Run a palette item (the window has closed by now, like the Tauri
/// palette's `runItem`).
fn run_palette_command(state: &Rc<RefCell<State>>, ui: &Ui, command: PaletteCommand) {
    match command {
        PaletteCommand::NewNote => create_note(state, ui),
        PaletteCommand::SaveNow => {
            save_now(state, ui, true);
        }
        PaletteCommand::OpenSettings => open_settings_or_focus(state, ui),
        PaletteCommand::OpenTrash => open_trash_or_focus(state, ui),
        PaletteCommand::RebuildIndex => rebuild_index(state, ui),
        PaletteCommand::TogglePin => toggle_pin(state, ui),
        PaletteCommand::OpenNote(id) => {
            open_note_by_id(state, ui, id);
            ui.editor.grab_focus();
        }
        PaletteCommand::ChooseFolder => choose_folder(state, ui),
    }
}

/// `workspace.rebuild_index()` — the Tauri palette's "Rebuild search index".
fn rebuild_index(state: &Rc<RefCell<State>>, ui: &Ui) {
    let outcome = {
        let mut st = state.borrow_mut();
        st.workspace
            .as_mut()
            .map(|ws| ws.rebuild_index().map_err(|error| error.to_string()))
    };
    match outcome {
        Some(Ok(stats)) => {
            refresh(state, ui, false);
            ui.toast.add_toast(adw::Toast::new(&format!(
                "Index rebuilt: {} added, {} updated, {} removed.",
                stats.added, stats.updated, stats.removed
            )));
        }
        Some(Err(error)) => {
            ui.toast
                .add_toast(adw::Toast::new(&format!("Rebuild failed: {error}")));
        }
        None => {}
    }
}

/// A settings row the "Search settings…" filter can hide; a group hides too
/// when none of its rows match.
struct Filterable {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    haystack: String,
}

/// Widgets of one shortcut row; the display repaints from the current
/// overrides whenever a binding changes.
struct ShortcutUi {
    id: &'static str,
    row: adw::ActionRow,
    binding: gtk::Label,
    change: gtk::Button,
    reset: gtk::Button,
}

fn track_row<W: IsA<gtk::Widget>>(
    filterables: &Rc<RefCell<Vec<Filterable>>>,
    widget: &W,
    group: &adw::PreferencesGroup,
    haystack: String,
) {
    filterables.borrow_mut().push(Filterable {
        widget: widget.clone().upcast(),
        group: group.clone(),
        haystack,
    });
}

/// The GTK settings panel: the subset of the Tauri registry this shell
/// honors. Every change is written to `config.json` immediately and applied
/// live, so there is no Save button — same as the Tauri panel.
fn open_settings_or_focus(state: &Rc<RefCell<State>>, ui: &Ui) {
    let Some(application) = ui.window.application() else {
        return;
    };
    if focus_window(&application, "Settings") {
        return;
    }
    open_settings(state, ui, &application);
}

fn open_settings(state: &Rc<RefCell<State>>, ui: &Ui, application: &gtk::Application) {
    let window = adw::Window::builder()
        .application(application)
        .title("Settings")
        .default_width(620)
        .default_height(560)
        .transient_for(&ui.window)
        .modal(true)
        .build();

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::new(Some("Settings"))));

    let filter = gtk::Entry::builder()
        .placeholder_text("Search settings…")
        .build();
    let filter_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    filter_box.set_margin_top(8);
    filter_box.set_margin_bottom(4);
    filter_box.set_margin_start(16);
    filter_box.set_margin_end(16);
    filter_box.append(&filter);
    let no_matches = gtk::Label::builder()
        .label("No matching settings")
        .css_classes(["dim-label"])
        .visible(false)
        .build();

    let page = adw::PreferencesPage::new();
    page.set_vexpand(true);
    let filterables: Rc<RefCell<Vec<Filterable>>> = Rc::new(RefCell::new(Vec::new()));

    // Editor: the autosave delay (the Tauri registry's 200–2000 ms slider).
    let editor_group = adw::PreferencesGroup::builder().title("Editor").build();
    let autosave = adw::SpinRow::with_range(200.0, 2000.0, 100.0);
    autosave.set_title("Autosave delay");
    autosave.set_subtitle("Pause after typing before saving, in milliseconds");
    autosave.set_value(state.borrow().autosave_ms as f64);
    {
        let state = state.clone();
        let ui = ui.clone();
        autosave.connect_value_notify(move |row| {
            let ms = row.value().round() as u64;
            if state.borrow().autosave_ms == ms {
                return;
            }
            state.borrow_mut().autosave_ms = ms;
            persist_setting(
                &state,
                &ui,
                "editor",
                "autosaveMs",
                serde_json::Value::from(ms),
            );
        });
    }
    editor_group.add(&autosave);
    track_row(
        &filterables,
        &autosave,
        &editor_group,
        "autosave delay editor pause typing save milliseconds".to_string(),
    );

    // View: word count and date format.
    let view_group = adw::PreferencesGroup::builder().title("View").build();
    let show_words = adw::SwitchRow::new();
    show_words.set_title("Show word count");
    show_words.set_subtitle("Shows the word count in the editor header");
    show_words.set_active(state.borrow().show_word_count);
    {
        let state = state.clone();
        let ui = ui.clone();
        show_words.connect_active_notify(move |row| {
            let active = row.is_active();
            if state.borrow().show_word_count == active {
                return;
            }
            state.borrow_mut().show_word_count = active;
            persist_setting(
                &state,
                &ui,
                "view",
                "showWordCount",
                serde_json::Value::Bool(active),
            );
            update_words(&state, &ui, &buffer_text(&ui.editor));
        });
    }
    view_group.add(&show_words);
    track_row(
        &filterables,
        &show_words,
        &view_group,
        "show word count view editor header".to_string(),
    );

    let date_format = adw::ComboRow::new();
    date_format.set_title("Date format");
    let formats = gtk::StringList::new(&["Relative (2h ago)", "Absolute (Oct 16, 14:00)"]);
    date_format.set_model(Some(&formats));
    let is_absolute = matches!(
        state.borrow().date_format,
        crate::dates::DateFormat::Absolute
    );
    date_format.set_selected(if is_absolute { 1 } else { 0 });
    {
        let state = state.clone();
        let ui = ui.clone();
        date_format.connect_selected_notify(move |row| {
            let absolute = row.selected() == 1;
            let current = matches!(
                state.borrow().date_format,
                crate::dates::DateFormat::Absolute
            );
            if current == absolute {
                return;
            }
            let format = if absolute {
                crate::dates::DateFormat::Absolute
            } else {
                crate::dates::DateFormat::Relative
            };
            state.borrow_mut().date_format = format;
            persist_setting(
                &state,
                &ui,
                "view",
                "dateFormat",
                serde_json::Value::from(if absolute { "absolute" } else { "relative" }),
            );
        });
    }
    view_group.add(&date_format);
    track_row(
        &filterables,
        &date_format,
        &view_group,
        "date format view relative absolute".to_string(),
    );

    // General: the delete confirmation.
    let general_group = adw::PreferencesGroup::builder().title("General").build();
    let confirm = adw::SwitchRow::new();
    confirm.set_title("Confirm before deleting");
    confirm.set_subtitle("Asks before a note moves to the trash");
    confirm.set_active(state.borrow().confirm_delete);
    {
        let state = state.clone();
        let ui = ui.clone();
        confirm.connect_active_notify(move |row| {
            let active = row.is_active();
            if state.borrow().confirm_delete == active {
                return;
            }
            state.borrow_mut().confirm_delete = active;
            persist_setting(
                &state,
                &ui,
                "general",
                "confirmDelete",
                serde_json::Value::Bool(active),
            );
        });
    }
    general_group.add(&confirm);
    track_row(
        &filterables,
        &confirm,
        &general_group,
        "confirm before deleting general trash".to_string(),
    );

    // Shortcuts: one row per keymap action, rebindable in place.
    let shortcuts_group = adw::PreferencesGroup::builder()
        .title("Shortcuts")
        .description(
            "Click Change, then press the keys. Backspace unbinds, Esc cancels. \
             Shortcuts cannot clash with another action or with text editing keys.",
        )
        .build();
    let shortcut_rows: Rc<RefCell<Vec<ShortcutUi>>> = Rc::new(RefCell::new(Vec::new()));
    let recording: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    for action in keymap::KEY_ACTIONS {
        let row = adw::ActionRow::new();
        row.set_title(keymap::label(action.id));
        let binding = gtk::Label::builder().css_classes(["dim-label"]).build();
        let reset = gtk::Button::with_label("Reset");
        reset.add_css_class("flat");
        reset.set_visible(false);
        let change = gtk::Button::with_label("Change");
        change.add_css_class("flat");
        row.add_suffix(&binding);
        row.add_suffix(&reset);
        row.add_suffix(&change);
        {
            let state = state.clone();
            let shortcut_rows = shortcut_rows.clone();
            let recording = recording.clone();
            change.connect_clicked(move |_| {
                let is_active = recording.borrow().as_deref() == Some(action.id);
                recording.replace(if is_active {
                    None
                } else {
                    Some(action.id.to_string())
                });
                refresh_shortcut_rows(&state, &shortcut_rows, &recording);
            });
        }
        {
            let state = state.clone();
            let ui = ui.clone();
            let shortcut_rows = shortcut_rows.clone();
            let recording = recording.clone();
            let application = application.clone();
            reset.connect_clicked(move |_| {
                let path = state.borrow().config_path.clone();
                if let Err(error) = config::remove_shortcut(&path, action.id) {
                    ui.toast.add_toast(adw::Toast::new(&format!(
                        "Could not save settings: {error}"
                    )));
                    return;
                }
                state.borrow_mut().overrides.remove(action.id);
                let overrides = state.borrow().overrides.clone();
                apply_accels(&application, &overrides, action.id);
                refresh_shortcut_rows(&state, &shortcut_rows, &recording);
            });
        }
        shortcuts_group.add(&row);
        shortcut_rows.borrow_mut().push(ShortcutUi {
            id: action.id,
            row: row.clone(),
            binding,
            change,
            reset,
        });
        track_row(
            &filterables,
            &row,
            &shortcuts_group,
            format!("shortcut {}", keymap::label(action.id)).to_lowercase(),
        );
    }
    refresh_shortcut_rows(state, &shortcut_rows, &recording);

    // Advanced: where state lives, rebuild the index, reset everything.
    let advanced_group = adw::PreferencesGroup::builder()
        .title("Advanced")
        .description("Config, index, trash and keys live here — never inside the vault.")
        .build();
    let data_row = adw::ActionRow::new();
    data_row.set_title("Data folder");
    data_row.set_subtitle(&state.borrow().data_dir.display().to_string());
    let open_data = gtk::Button::with_label("Open");
    open_data.add_css_class("flat");
    {
        let ui = ui.clone();
        let window = window.clone();
        let path = state.borrow().data_dir.clone();
        open_data.connect_clicked(move |_| {
            let uri = gtk::gio::File::for_path(&path).uri();
            gtk::UriLauncher::new(uri.as_str()).launch(
                Some(&window),
                gtk::gio::Cancellable::NONE,
                {
                    let ui = ui.clone();
                    move |result| {
                        if let Err(error) = result {
                            ui.toast.add_toast(adw::Toast::new(&format!(
                                "Could not open the data folder: {error}"
                            )));
                        }
                    }
                },
            );
        });
    }
    data_row.add_suffix(&open_data);
    advanced_group.add(&data_row);
    track_row(
        &filterables,
        &data_row,
        &advanced_group,
        "data folder advanced config index trash keys".to_string(),
    );

    let rebuild_row = adw::ActionRow::new();
    rebuild_row.set_title("Search index");
    rebuild_row.set_subtitle("Rebuildable from the Markdown files at any time.");
    let rebuild_button = gtk::Button::with_label("Rebuild now");
    rebuild_button.add_css_class("flat");
    rebuild_button.set_sensitive(state.borrow().workspace.is_some());
    {
        let state = state.clone();
        let ui = ui.clone();
        rebuild_button.connect_clicked(move |_| rebuild_index(&state, &ui));
    }
    rebuild_row.add_suffix(&rebuild_button);
    advanced_group.add(&rebuild_row);
    track_row(
        &filterables,
        &rebuild_row,
        &advanced_group,
        "search index rebuild advanced maintenance".to_string(),
    );

    let reset_row = adw::ActionRow::new();
    reset_row.set_title("Reset all settings");
    reset_row.set_subtitle("Back to defaults. Notes are not touched.");
    let reset_button = gtk::Button::with_label("Reset settings");
    reset_button.add_css_class("destructive-action");
    let armed: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    {
        let state = state.clone();
        let ui = ui.clone();
        let window = window.clone();
        let application = application.clone();
        let armed = armed.clone();
        reset_button.connect_clicked(move |button| {
            if !*armed.borrow() {
                armed.replace(true);
                button.set_label("Confirm reset");
                let button = button.clone();
                let armed = armed.clone();
                gtk::glib::timeout_add_seconds_local(3, move || {
                    armed.replace(false);
                    button.set_label("Reset settings");
                    gtk::glib::ControlFlow::Break
                });
                return;
            }
            armed.replace(false);
            let path = state.borrow().config_path.clone();
            if let Err(error) = config::clear_settings(&path) {
                ui.toast.add_toast(adw::Toast::new(&format!(
                    "Could not save settings: {error}"
                )));
                return;
            }
            {
                let mut st = state.borrow_mut();
                st.autosave_ms = config::DEFAULT_AUTOSAVE_MS;
                st.confirm_delete = true;
                st.show_word_count = true;
                st.date_format = crate::dates::DateFormat::Relative;
                st.overrides.clear();
            }
            let overrides = state.borrow().overrides.clone();
            apply_all_accels(&application, &overrides);
            update_words(&state, &ui, &buffer_text(&ui.editor));
            window.close();
            ui.toast.add_toast(adw::Toast::new(
                "All settings were reset to their defaults.",
            ));
        });
    }
    reset_row.add_suffix(&reset_button);
    advanced_group.add(&reset_row);
    track_row(
        &filterables,
        &reset_row,
        &advanced_group,
        "reset all settings advanced defaults".to_string(),
    );

    page.add(&editor_group);
    page.add(&view_group);
    page.add(&general_group);
    page.add(&shortcuts_group);
    page.add(&advanced_group);

    // One capture-phase controller: Esc closes the window; while a shortcut
    // records, Esc cancels and every other press is stored, refused with a
    // reason, or swallowed (modifiers).
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let state = state.clone();
        let window_for_escape = window.clone();
        let shortcut_rows = shortcut_rows.clone();
        let recording = recording.clone();
        let application = application.clone();
        keys.connect_key_pressed(move |_, key, _code, modifiers| {
            let name = key.name().map(|name| name.to_string()).unwrap_or_default();
            let Some(active_id) = recording.borrow().clone() else {
                if name == "Escape" {
                    window_for_escape.close();
                    return gtk::glib::Propagation::Stop;
                }
                return gtk::glib::Propagation::Proceed;
            };
            if name == "Escape" {
                recording.replace(None);
                refresh_shortcut_rows(&state, &shortcut_rows, &recording);
                return gtk::glib::Propagation::Stop;
            }
            if keymap::is_modifier_key(&name) {
                return gtk::glib::Propagation::Stop;
            }
            if name == "BackSpace" || name == "Delete" {
                let path = state.borrow().config_path.clone();
                if let Err(error) = config::set_shortcut(&path, &active_id, None) {
                    set_recording_error(
                        &shortcut_rows,
                        &active_id,
                        &format!("Could not save settings: {error}"),
                    );
                    return gtk::glib::Propagation::Stop;
                }
                state.borrow_mut().overrides.insert(active_id.clone(), None);
                let overrides = state.borrow().overrides.clone();
                apply_accels(&application, &overrides, &active_id);
                recording.replace(None);
                refresh_shortcut_rows(&state, &shortcut_rows, &recording);
                return gtk::glib::Propagation::Stop;
            }
            let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                || modifiers.contains(gtk::gdk::ModifierType::META_MASK);
            let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);
            let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            let Some(binding) = keymap::binding_from_parts(ctrl, alt, shift, &name) else {
                set_recording_error(
                    &shortcut_rows,
                    &active_id,
                    "Include Ctrl or Alt in the shortcut.",
                );
                return gtk::glib::Propagation::Stop;
            };
            if keymap::is_reserved(&binding) {
                set_recording_error(
                    &shortcut_rows,
                    &active_id,
                    &format!("{binding} is reserved for text editing."),
                );
                return gtk::glib::Propagation::Stop;
            }
            if let Some(other) =
                keymap::binding_conflict(&active_id, &binding, &state.borrow().overrides)
            {
                set_recording_error(
                    &shortcut_rows,
                    &active_id,
                    &format!("{binding} is already used by {}.", keymap::label(other)),
                );
                return gtk::glib::Propagation::Stop;
            }
            let path = state.borrow().config_path.clone();
            if let Err(error) = config::set_shortcut(&path, &active_id, Some(&binding)) {
                set_recording_error(
                    &shortcut_rows,
                    &active_id,
                    &format!("Could not save settings: {error}"),
                );
                return gtk::glib::Propagation::Stop;
            }
            state
                .borrow_mut()
                .overrides
                .insert(active_id.clone(), Some(binding));
            let overrides = state.borrow().overrides.clone();
            apply_accels(&application, &overrides, &active_id);
            recording.replace(None);
            refresh_shortcut_rows(&state, &shortcut_rows, &recording);
            gtk::glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    {
        let filterables = filterables.clone();
        let no_matches = no_matches.clone();
        filter.connect_changed(move |entry| {
            let query = entry.text().trim().to_lowercase();
            let items = filterables.borrow();
            let mut groups: Vec<adw::PreferencesGroup> = Vec::new();
            let mut any_visible = false;
            for item in items.iter() {
                if !groups.iter().any(|group| group == &item.group) {
                    groups.push(item.group.clone());
                }
                let matches = query.is_empty() || item.haystack.contains(&query);
                item.widget.set_visible(matches);
                any_visible |= matches;
            }
            for group in &groups {
                let group_visible = items.iter().any(|item| {
                    &item.group == group && (query.is_empty() || item.haystack.contains(&query))
                });
                group.set_visible(group_visible);
            }
            no_matches.set_visible(!any_visible);
        });
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&filter_box);
    content.append(&no_matches);
    content.append(&page);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));

    window.present();
    filter.grab_focus();
}

/// Repaint every shortcut row from the current overrides: the binding text,
/// the Change/Cancel button and the Reset link.
fn refresh_shortcut_rows(
    state: &Rc<RefCell<State>>,
    rows: &Rc<RefCell<Vec<ShortcutUi>>>,
    recording: &Rc<RefCell<Option<String>>>,
) {
    let active = recording.borrow().clone();
    for shortcut in rows.borrow().iter() {
        let binding = keymap::hint(shortcut.id, &state.borrow().overrides);
        let is_recording = active.as_deref() == Some(shortcut.id);
        shortcut.binding.set_text(if is_recording {
            "Press keys…"
        } else if binding.is_empty() {
            "Not set"
        } else {
            binding.as_str()
        });
        shortcut
            .change
            .set_label(if is_recording { "Cancel" } else { "Change" });
        shortcut
            .reset
            .set_visible(state.borrow().overrides.contains_key(shortcut.id) && !is_recording);
        shortcut.row.set_subtitle(if is_recording {
            "Press the keys. Backspace unbinds, Esc cancels."
        } else {
            ""
        });
    }
}

/// Put a reason under the row that is recording.
fn set_recording_error(rows: &Rc<RefCell<Vec<ShortcutUi>>>, id: &str, message: &str) {
    if let Some(shortcut) = rows.borrow().iter().find(|shortcut| shortcut.id == id) {
        shortcut.row.set_subtitle(message);
    }
}

/// Write one option to `config.json`; a failure only toasts — the in-memory
/// change stands for this session.
fn persist_setting(
    state: &Rc<RefCell<State>>,
    ui: &Ui,
    section: &str,
    key: &str,
    value: serde_json::Value,
) {
    let path = state.borrow().config_path.clone();
    if let Err(error) = config::set_setting(&path, section, key, value) {
        ui.toast.add_toast(adw::Toast::new(&format!(
            "Could not save settings: {error}"
        )));
    }
}

/// Widgets of the open command palette; the rows are rebuilt as the query
/// changes, `commands` mirrors them in row order.
#[derive(Clone)]
struct Palette {
    entry: gtk::Entry,
    list: gtk::ListBox,
    empty: gtk::Label,
    commands: Rc<RefCell<Vec<PaletteItem>>>,
}

/// One palette row: what it shows and what it runs.
struct PaletteItem {
    label: String,
    hint: String,
    command: PaletteCommand,
}

/// What a palette item does; `Clone` so a copy can run after the window
/// closed.
#[derive(Clone)]
enum PaletteCommand {
    NewNote,
    SaveNow,
    OpenSettings,
    OpenTrash,
    RebuildIndex,
    TogglePin,
    OpenNote(NoteId),
    ChooseFolder,
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
