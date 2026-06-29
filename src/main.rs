use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

mod app;
mod audio;
mod config;
mod dictation;
mod llm;
mod note;
mod search;
mod server;
mod setup;
mod storage;
#[cfg(test)]
mod testutil;
mod theme;
mod ui;
mod update;

use app::App;
use config::Config;
use storage::Store;

#[derive(Parser)]
#[command(
    name = "memo",
    version,
    about = "Terminal AI note-taker that refines notes with a local LLM (MLX or Ollama)"
)]
struct Cli {
    /// Re-run the local LLM server setup (choose MLX or Ollama) and update the config.
    #[arg(long)]
    setup: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show or edit the configuration file
    Config {
        /// Print the path to the config file
        #[arg(long)]
        path: bool,
        /// Open the config file in $EDITOR
        #[arg(long)]
        edit: bool,
    },
    /// Update memo to the latest release.
    Update {
        /// Include pre-releases (rc / alpha / beta), not just stable versions.
        #[arg(long)]
        pre: bool,
        /// Only check whether an update is available; don't install it.
        #[arg(long)]
        check: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.setup {
        return setup::run();
    }
    match cli.command {
        Some(Commands::Config { path, edit }) => run_config(path, edit),
        Some(Commands::Update { pre, check }) => update::run(pre, check),
        None => run_tui(),
    }
}

fn run_config(path: bool, edit: bool) -> Result<()> {
    // Ensure the file exists (creates it with defaults if needed).
    let _ = Config::load_or_create()?;
    let cfg_path = config::config_path();

    if path {
        println!("{}", cfg_path.display());
        return Ok(());
    }
    if edit {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        std::process::Command::new(editor).arg(&cfg_path).status()?;
        return Ok(());
    }
    println!("# {}", cfg_path.display());
    print!("{}", std::fs::read_to_string(&cfg_path)?);
    Ok(())
}

fn run_tui() -> Result<()> {
    let cfg = Config::load_or_create()?;
    let data_dir = cfg.resolved_data_dir()?;
    let store = Store::new(data_dir)?;

    let mut app = App::new(cfg, store);
    // Start the local model server now in load-at-startup mode; on-demand mode
    // defers it to the first refine. The app owns the guard and stops it on quit.
    app.start_managed_server_if_eager();
    // Prefetch the speech model in the background now (download it if missing,
    // then load it) so dictation just works by the time the user gets to it —
    // even on a fresh install. Runs on the dictation worker thread; silent.
    app.prefetch_dictation();

    let (mut terminal, release_seed) = setup_terminal()?;
    // Seed the gesture recognizer; it upgrades to "releases supported" on the first
    // real release event regardless of this guess.
    app.set_release_supported(release_seed);
    let result = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;

    // Stop the managed server now that the terminal is restored (no-op if none is
    // running, e.g. on-demand mode already shut it down). Synchronous so it is
    // signalled before the process exits.
    app.stop_managed_server();
    // Join the dictation worker so the whisper.cpp Metal context is fully torn
    // down before C++ atexit handlers run — avoids a Metal assertion on exit.
    app.join_dictation_thread();
    result
}

/// Sets up the terminal and returns it along with the best initial guess at
/// whether key Press/Repeat/Release events will be reported (the Kitty keyboard
/// protocol, needed for hold-to-talk dictation).
///
/// We push the enhancement flag *unconditionally* rather than gating on the
/// capability query: that query is unreliable in some terminals (it can answer
/// "no" even where releases work, e.g. some Ghostty setups), and the push is a
/// CSI sequence terminals that don't support it simply ignore. The returned bool
/// is only a seed — `App` upgrades it to true the moment a real release event
/// arrives (see `App::on_key_event`).
fn setup_terminal() -> Result<(Terminal<CrosstermBackend<Stdout>>, bool)> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let release_seed = supports_keyboard_enhancement().unwrap_or(false);
    // Only REPORT_EVENT_TYPES — enough to distinguish Press/Repeat/Release without
    // REPORT_ALL_KEYS_AS_ESCAPE_CODES, which would change how existing keys
    // (Tab, Esc, Alt+\, Ctrl combos) are reported.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
    );
    set_panic_hook();
    Ok((Terminal::new(CrosstermBackend::new(stdout))?, release_seed))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    // Pop unconditionally — we always push; popping when nothing was pushed (or on
    // a terminal that ignored the push) is a harmless no-op.
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    let _ = execute!(terminal.backend_mut(), DisableBracketedPaste);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Restore the terminal on panic so the user isn't left in a broken alt-screen.
fn set_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original(info);
    }));
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    // Set NOTE_DEBUG_KEYS=1 to log every key event (kind/code/modifiers) to
    // <config_dir>/keys-debug.log — handy for diagnosing whether the terminal
    // delivers Release events for dictation.
    let debug_keys = std::env::var_os("NOTE_DEBUG_KEYS").is_some();
    let mut last_view = view_key(app);
    while !app.should_quit {
        // On a view change (screen, modal, drawer, or folder), force a full
        // repaint. ratatui only rewrites cells it thinks changed, so a glyph the
        // terminal drew wider than its cell can leave a "ghost" half-cell that the
        // diff never clears; clearing on transitions wipes those stale cells.
        let view = view_key(app);
        if view != last_view {
            terminal.clear()?;
            last_view = view;
        }
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if debug_keys {
                        log_key_event(&key);
                    }
                    // Forward every kind (Press/Repeat/Release). on_key_event routes
                    // the dictation key to the gesture recognizer and everything else
                    // through the normal Press/Repeat dispatch.
                    app.on_key_event(key);
                }
                // A clipboard paste (bracketed paste) arrives as one event with the
                // full text — insert it directly so multi-line pastes are instant.
                Event::Paste(text) => app.on_paste(text),
                _ => {}
            }
        }
        app.on_tick();
    }
    Ok(())
}

/// Append a key event to the debug log (enabled via NOTE_DEBUG_KEYS).
fn log_key_event(key: &event::KeyEvent) {
    use std::io::Write;
    let path = config::config_dir().join("keys-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            f,
            "{:?} code={:?} mods={:?}",
            key.kind, key.code, key.modifiers
        );
    }
}

/// Identity of the current view; a change between frames triggers a full repaint.
fn view_key(app: &App) -> (crate::app::Screen, bool, bool, Option<String>) {
    (
        app.screen,
        !matches!(app.modal, crate::app::Modal::None),
        app.drawer_open,
        app.current_folder.clone(),
    )
}

#[cfg(test)]
mod smoke {
    //! Headless rendering tests: drive the app with synthetic key events against a
    //! `TestBackend` and assert the draw + update paths never panic.
    use super::*;
    use crate::app::{EditorView, Focus, ListItem, RefineStatus};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use std::io::Write;
    use std::net::TcpListener;
    use std::time::Duration;

    fn test_app() -> App {
        let dir = std::env::temp_dir().join(format!("ant-ui-{}", uuid::Uuid::new_v4()));
        let store = Store::new(dir).unwrap();
        App::new(Config::default(), store)
    }

    /// App wired to a looping mock chat-completions server that always returns `body`.
    fn app_with_server(body: &'static str) -> App {
        app_with_server_cfg(body, |_| {})
    }

    /// Like [`app_with_server`], but `tweak` can adjust the config first (e.g. to
    /// switch on on-demand mode). `auto_start_server` is forced off so a test never
    /// launches — or adopts via the pid file — a real model server: the mock here
    /// *is* the server.
    fn app_with_server_cfg(body: &'static str, tweak: impl FnOnce(&mut Config)) -> App {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut stream = conn;
                crate::testutil::drain_http_request(&mut stream);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        let dir = std::env::temp_dir().join(format!("ant-refine-{}", uuid::Uuid::new_v4()));
        let store = Store::new(dir).unwrap();
        let mut cfg = Config {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            request_timeout_secs: 5,
            auto_start_server: false,
            ..Config::default()
        };
        tweak(&mut cfg);
        App::new(cfg, store)
    }

    fn wait_for_refine(app: &mut App) {
        for _ in 0..300 {
            app.on_tick();
            if app.refine_status() != RefineStatus::Refining {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("refine did not complete in time");
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            if c == '\n' {
                app.on_key(code(KeyCode::Enter));
            } else {
                app.on_key(key(c));
            }
        }
    }

    /// Clear a prefilled modal input buffer.
    fn clear_input(app: &mut App) {
        for _ in 0..80 {
            app.on_key(code(KeyCode::Backspace));
        }
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn code(k: KeyCode) -> KeyEvent {
        KeyEvent::new(k, KeyModifiers::NONE)
    }

    fn render(app: &mut App) {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
    }

    #[test]
    fn drives_all_screens_without_panicking() {
        let mut app = test_app();
        render(&mut app); // empty list

        app.on_key(ctrl('h')); // help modal (Ctrl+H)
        render(&mut app);
        app.on_key(code(KeyCode::Esc)); // close help

        app.on_key(key('n')); // new note -> editor
        render(&mut app);
        for c in "# Title\n- item".chars() {
            if c == '\n' {
                app.on_key(code(KeyCode::Enter));
            } else {
                app.on_key(key(c));
            }
        }
        render(&mut app);

        app.on_key(ctrl('t')); // edit-title modal
        render(&mut app);
        app.on_key(code(KeyCode::Enter));

        app.on_key(ctrl('x')); // export modal
        render(&mut app);
        app.on_key(code(KeyCode::Esc));

        app.on_key(code(KeyCode::Esc)); // back to list (persists the note)
        render(&mut app);
        assert_eq!(app.notes.len(), 1);

        app.on_key(key('/')); // search
        app.on_key(key('T'));
        render(&mut app);
        // The note title contains "T" (from the auto-titled "Untitled …"), so
        // at least one tile remains visible; searching for something absent clears the list.
        assert!(!app.items.is_empty(), "a matching note should remain visible");
        app.on_key(code(KeyCode::Esc)); // cancel search — restores all tiles
        assert_eq!(app.items.len(), 1, "all notes visible again after Esc");
    }

    #[test]
    fn search_opens_highlighted_note_and_dismisses_cleanly() {
        let mut app = test_app();
        // Two notes so an active search visibly filters the grid. "alph" matches
        // only "alpha" ("beta" ends in 'a', so a bare "a" would match both).
        for body in ["alpha", "beta"] {
            app.on_key(key('n'));
            typed(&mut app, body);
            app.on_key(code(KeyCode::Esc));
        }
        assert_eq!(app.items.len(), 2);

        // `/` opens search; a second `/` dismisses it immediately.
        app.on_key(key('/'));
        assert!(matches!(app.focus, Focus::Search));
        app.on_key(key('/'));
        assert!(matches!(app.focus, Focus::Tiles));
        assert!(app.search_query.is_empty());
        assert_eq!(app.items.len(), 2);

        // Esc from the search bar also dismisses — and must not prompt to quit.
        app.on_key(key('/'));
        typed(&mut app, "alph");
        assert_eq!(app.search_query, "alph");
        assert_eq!(app.items.len(), 1, "search filters to the matching note");
        app.on_key(code(KeyCode::Esc));
        assert!(matches!(app.focus, Focus::Tiles));
        assert!(app.search_query.is_empty(), "Esc clears the search query");
        assert!(
            matches!(app.modal, crate::app::Modal::None),
            "Esc on an active search must dismiss it, not open the quit prompt"
        );
        assert!(!app.should_quit);
        assert_eq!(app.items.len(), 2, "all notes visible again after Esc");

        // Enter from the search bar opens the highlighted result directly...
        app.on_key(key('/'));
        typed(&mut app, "alph");
        assert_eq!(app.items.len(), 1);
        app.on_key(code(KeyCode::Enter));
        assert!(app.editing.is_some(), "Enter opens the highlighted note");
        assert_eq!(app.editing.as_ref().unwrap().content, "alpha");

        // ...and returning from it dismisses the search in the background: the
        // list shows the full, unfiltered set with no query left active.
        app.on_key(code(KeyCode::Esc));
        assert!(app.editing.is_none());
        assert!(
            app.search_query.is_empty(),
            "search is gone after returning from a note"
        );
        assert!(matches!(app.focus, Focus::Tiles));
        assert_eq!(app.items.len(), 2);
    }

    #[test]
    fn refine_auto_titles_and_makes_refined_editable() {
        let mut app = app_with_server(
            r##"{"choices":[{"message":{"content":"# Groceries\n- milk\n- eggs"}}]}"##,
        );

        app.on_key(key('n')); // new note
        typed(&mut app, "buy milk and eggs");
        app.on_key(ctrl('r')); // refine
        wait_for_refine(&mut app);

        assert_eq!(app.refine_status(), RefineStatus::Done);
        let note = app.editing.as_ref().unwrap();
        assert_eq!(note.refined.as_deref(), Some("# Groceries\n- milk\n- eggs"));
        assert_eq!(note.meta.title, "Groceries"); // auto-titled from the refined heading
        assert!(!note.meta.title_custom);
        assert!(matches!(app.editor_view, EditorView::Refined));
        // The refined view is editable: its text is loaded into the editor textarea.
        assert_eq!(
            app.textarea.lines().join("\n"),
            "# Groceries\n- milk\n- eggs"
        );
        render(&mut app);

        // Edit the refined text and toggle to original and back — edits must persist.
        typed(&mut app, "\n- bread");
        app.on_key(code(KeyCode::Tab)); // -> original
        assert!(matches!(app.editor_view, EditorView::Original));
        assert_eq!(app.editing.as_ref().unwrap().content, "buy milk and eggs");
        app.on_key(code(KeyCode::Tab)); // -> refined
        assert!(app.textarea.lines().join("\n").contains("- bread"));
    }

    #[test]
    fn refine_keeps_user_set_title() {
        let mut app =
            app_with_server(r##"{"choices":[{"message":{"content":"# Auto Title\nbody"}}]}"##);

        app.on_key(key('n'));
        typed(&mut app, "some content");
        // User sets a custom title (clear the prefilled default first).
        app.on_key(ctrl('t'));
        clear_input(&mut app);
        typed(&mut app, "My Title");
        app.on_key(code(KeyCode::Enter));
        assert!(app.editing.as_ref().unwrap().meta.title_custom);
        assert_eq!(app.editing.as_ref().unwrap().meta.title, "My Title");

        app.on_key(ctrl('r')); // refine
        wait_for_refine(&mut app);

        // Title must NOT be overwritten by the refined heading.
        assert_eq!(app.editing.as_ref().unwrap().meta.title, "My Title");
    }

    #[test]
    fn on_demand_mode_refines_and_ticks_cleanly() {
        // With server_on_demand on (and a zero idle window so shutdown is immediate),
        // refining still works against an already-running server, and the post-refine
        // idle-shutdown path runs without panicking. `note` manages no process here
        // (auto_start_server is forced off in the helper), so nothing is launched.
        let mut app = app_with_server_cfg(
            r##"{"choices":[{"message":{"content":"# Done\n- ok"}}]}"##,
            |cfg| {
                cfg.server_on_demand = true;
                cfg.server_idle_timeout_secs = 0;
            },
        );

        app.on_key(key('n'));
        typed(&mut app, "raw note");
        app.on_key(ctrl('r'));
        wait_for_refine(&mut app);

        assert_eq!(app.refine_status(), RefineStatus::Done);
        assert_eq!(
            app.editing.as_ref().unwrap().refined.as_deref(),
            Some("# Done\n- ok")
        );
        // The idle-shutdown bookkeeping (a no-op when no server is owned) must keep
        // the app healthy across ticks.
        app.on_tick();
        assert!(!app.should_quit);
    }

    #[test]
    fn autosaves_after_debounce() {
        let dir = std::env::temp_dir().join(format!("ant-autosave-{}", uuid::Uuid::new_v4()));
        let store = Store::new(dir.clone()).unwrap();
        let mut app = App::new(Config::default(), store);

        app.on_key(key('n'));
        typed(&mut app, "autosaved content");
        // Never press Ctrl+S or Esc; just let the debounce elapse and tick.
        std::thread::sleep(Duration::from_millis(850));
        app.on_tick();

        let reloaded = Store::new(dir).unwrap().list().unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].content, "autosaved content");
    }

    #[test]
    fn create_folder_enter_and_add_note() {
        let mut app = test_app();

        // Ctrl+F opens the new-folder prompt; type a title and confirm.
        app.on_key(ctrl('f'));
        typed(&mut app, "Work");
        app.on_key(code(KeyCode::Enter));
        assert_eq!(app.folders.len(), 1);
        assert_eq!(app.folders[0].title, "Work");
        let fid = app.folders[0].id.clone();
        // The folder shows as a tile at the top level.
        assert!(matches!(app.items.first(), Some(ListItem::Folder(_))));
        render(&mut app);

        // Enter the folder, then create a note inside it.
        app.on_key(code(KeyCode::Enter));
        assert_eq!(app.current_folder.as_deref(), Some(fid.as_str()));
        app.on_key(key('n'));
        typed(&mut app, "inside the folder");
        app.on_key(code(KeyCode::Esc)); // save & back to the folder view

        assert_eq!(app.notes.len(), 1);
        assert_eq!(app.notes[0].meta.folder.as_deref(), Some(fid.as_str()));
        assert!(matches!(app.items.first(), Some(ListItem::Note(_))));
        render(&mut app);

        // Esc leaves the folder; the note now lives inside it, not at the top level.
        app.on_key(code(KeyCode::Esc));
        assert!(app.current_folder.is_none());
        assert_eq!(app.items.len(), 1);
        assert!(matches!(app.items[0], ListItem::Folder(_)));
        assert!(!app.should_quit);
    }

    #[test]
    fn move_loose_note_into_folder() {
        let mut app = test_app();

        // A loose note at the top level.
        app.on_key(key('n'));
        typed(&mut app, "loose note");
        app.on_key(code(KeyCode::Esc));
        assert_eq!(app.notes.len(), 1);

        // A folder to move it into.
        app.on_key(ctrl('f'));
        typed(&mut app, "Archive");
        app.on_key(code(KeyCode::Enter));
        let fid = app.folders[0].id.clone();

        // Items are folders-first: [Folder, Note]. Select the note, then move it.
        app.on_key(key('j')); // down to the note
        assert!(matches!(app.items[app.selected], ListItem::Note(_)));
        app.on_key(key('m')); // open the move picker
        app.on_key(code(KeyCode::Down)); // None -> Archive
        app.on_key(code(KeyCode::Enter)); // confirm

        assert_eq!(app.notes[0].meta.folder.as_deref(), Some(fid.as_str()));
        // The note is now inside the folder, so the top level shows only the folder.
        assert_eq!(app.items.len(), 1);
        assert!(matches!(app.items[0], ListItem::Folder(_)));
    }

    #[test]
    fn drawer_expands_folder_and_opens_a_nested_note() {
        let mut app = test_app();

        // A folder with one note inside, plus a loose note at the top level.
        app.on_key(ctrl('f'));
        typed(&mut app, "Work");
        app.on_key(code(KeyCode::Enter));
        let fid = app.folders[0].id.clone();
        app.on_key(code(KeyCode::Enter)); // enter "Work"
        app.on_key(key('n'));
        typed(&mut app, "nested note");
        app.on_key(code(KeyCode::Esc)); // save in folder
        app.on_key(code(KeyCode::Esc)); // back to the top level
        app.on_key(key('n'));
        typed(&mut app, "loose note");
        app.on_key(code(KeyCode::Esc));
        assert_eq!(app.notes.len(), 2);

        // d opens the drawer and focuses it. Tree: [Folder Work] [loose note].
        app.on_key(key('d'));
        assert!(app.drawer_open);
        render(&mut app);

        // Expand the folder (cursor starts on it), then step onto its nested note.
        app.on_key(code(KeyCode::Right));
        assert!(app.expanded.contains(&fid));
        render(&mut app);
        app.on_key(code(KeyCode::Down));
        app.on_key(code(KeyCode::Enter)); // open the nested note

        let editing = app.editing.as_ref().expect("a note should be open");
        assert_eq!(editing.meta.folder.as_deref(), Some(fid.as_str()));
        assert!(app.drawer_open); // stays open beside the editor
        render(&mut app);
    }

    #[test]
    fn drawer_toggle_is_home_screen_only() {
        let mut app = test_app();
        app.on_key(key('n'));
        typed(&mut app, "note one");
        app.on_key(code(KeyCode::Esc));

        // On the home screen, d opens and closes the drawer.
        app.on_key(key('d'));
        assert!(app.drawer_open);
        app.on_key(key('d')); // drawer focused -> d on a leaf closes it
        assert!(!app.drawer_open);

        // In the editor, d is ordinary typed text, not a toggle.
        app.on_key(code(KeyCode::Enter)); // open the note
        let before = app.drawer_open;
        app.on_key(key('d'));
        assert_eq!(app.drawer_open, before);
        assert!(app.textarea.lines().join("\n").contains('d'));
    }

    #[test]
    fn new_note_works_while_drawer_is_open() {
        let mut app = test_app();
        app.on_key(key('n'));
        typed(&mut app, "first");
        app.on_key(code(KeyCode::Esc));
        assert_eq!(app.notes.len(), 1);

        // Open and focus the drawer.
        app.on_key(key('d'));
        assert!(app.drawer_open);
        assert!(matches!(app.focus, Focus::Drawer));

        // `n` must still create a new note (and enter the editor) with the drawer
        // focused — global commands aren't swallowed by the drawer.
        app.on_key(key('n'));
        assert!(
            matches!(app.screen, crate::app::Screen::Editor),
            "n should open the editor even with the drawer focused"
        );
        assert!(app.editing.is_some());
        typed(&mut app, "from the drawer");
        app.on_key(code(KeyCode::Esc));
        assert_eq!(app.notes.len(), 2, "a second note was created from the drawer");
    }

    #[test]
    fn paste_inserts_multiline_text_into_the_note() {
        let mut app = test_app();
        app.on_key(key('n')); // new note -> editor
        app.on_paste("line one\nline two".to_string());
        assert_eq!(app.textarea.lines().join("\n"), "line one\nline two");
    }

    #[test]
    fn paste_normalizes_crlf_in_the_note() {
        // A CRLF paste is normalized — no stray carriage returns end up in the note.
        let mut app = test_app();
        app.on_key(key('n'));
        app.on_paste("a\r\nb".to_string());
        assert_eq!(app.textarea.lines().join("\n"), "a\nb");
    }

    #[test]
    fn paste_into_search_filters_notes() {
        let mut app = test_app();
        for body in ["alpha", "beta"] {
            app.on_key(key('n'));
            typed(&mut app, body);
            app.on_key(code(KeyCode::Esc));
        }
        app.on_key(key('/')); // focus the search bar
        app.on_paste("alph".to_string());
        assert_eq!(app.search_query, "alph");
        assert_eq!(app.items.len(), 1, "paste into search filters to the match");
    }

    #[test]
    fn paste_into_title_modal_is_flattened_to_one_line() {
        let mut app = test_app();
        app.on_key(key('n'));
        app.on_key(ctrl('t')); // title-edit modal (prefilled)
        clear_input(&mut app); // clear the prefilled default
        app.on_paste("New\nMulti\nLine".to_string());
        app.on_key(code(KeyCode::Enter)); // commit the title
        assert_eq!(app.editing.as_ref().unwrap().meta.title, "New Multi Line");
    }
}
