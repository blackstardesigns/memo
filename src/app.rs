use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Style;
use tui_textarea::TextArea;

use crate::config::{self, Config};
use crate::dictation::{self, DictationCmd, DictationMsg, DictationStatus};
use crate::llm::{self, RefineMsg};
use crate::note::{self, Folder, Note};
use crate::search;
use crate::server::ManagedServer;
use crate::storage::{self, Store};
use crate::theme::ResolvedTheme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    List,
    Editor,
}

/// One cell in the list grid: either a folder or a note. Both render as a tile
/// of the same size; the index points into [`App::folders`] / [`App::notes`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListItem {
    Folder(usize),
    Note(usize),
}

/// Where keyboard input is directed within the list screen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tiles,
    Search,
    /// The left drawer (notes/folders tree) has the keyboard.
    Drawer,
}

/// One visible row of the drawer's notes/folder tree. Indices point into
/// [`App::folders`] / [`App::notes`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DrawerRow {
    Folder {
        index: usize,
        expanded: bool,
        count: usize,
    },
    /// A note row. `child` is true when it's nested under an expanded folder.
    Note { index: usize, child: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorView {
    Original,
    Refined,
}

/// State of the LLM refinement, shown in the bottom-right corner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RefineStatus {
    Idle,
    Refining,
    Done,
    Error,
}

/// How long after the last keystroke before the editor autosaves.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(800);

/// Mic RMS that maps to a full-height, fully-red listening cursor. Normal speech
/// sits well below this, so the meter spends most of its travel on quieter input.
const LEVEL_FULL_SCALE: f32 = 0.15;
/// Per-tick multiplier for the input-level meter's decay (the envelope "release").
const LEVEL_RELEASE: f32 = 0.55;

/// Modal overlays. The variants carrying a `String` hold an editable input buffer.
pub enum Modal {
    None,
    Help,
    ConfirmDelete,
    /// Confirm deleting a folder (by id); its notes are moved back to the top level.
    ConfirmDeleteFolder(String),
    Error(String),
    Info(String),
    TitleEdit {
        buf: String,
        cursor: usize,
    },
    Export(String),
    /// Prompt for a new folder's title.
    NewFolder(String),
    /// Pick a destination folder for a note. `sel` is the highlighted row:
    /// 0 = top level, 1..=folders.len() = that folder.
    MoveNote {
        note_id: String,
        sel: usize,
    },
    ConfirmQuit,
    /// One-shot custom system prompt that overrides `cfg.refine_prompt` for a
    /// single refine call; not persisted anywhere.
    CustomPrompt(String),
    /// Searchable Unicode math / equation symbol picker. `query` filters by symbol
    /// name; `sel` is the highlighted row in the filtered list.
    SymbolPicker { query: String, sel: usize },
}

/// All available math / equation symbols, each as `(character, searchable names)`.
/// Names are space-separated keywords; the filter matches any substring.
pub const SYMBOLS: &[(char, &str)] = &[
    // Greek lowercase
    ('α', "alpha"),
    ('β', "beta"),
    ('γ', "gamma"),
    ('δ', "delta"),
    ('ε', "epsilon"),
    ('ζ', "zeta"),
    ('η', "eta"),
    ('θ', "theta"),
    ('ι', "iota"),
    ('κ', "kappa"),
    ('λ', "lambda"),
    ('μ', "mu"),
    ('ν', "nu"),
    ('ξ', "xi"),
    ('π', "pi"),
    ('ρ', "rho"),
    ('σ', "sigma"),
    ('τ', "tau"),
    ('υ', "upsilon"),
    ('φ', "phi"),
    ('χ', "chi"),
    ('ψ', "psi"),
    ('ω', "omega"),
    // Greek uppercase
    ('Γ', "Gamma"),
    ('Δ', "Delta"),
    ('Θ', "Theta"),
    ('Λ', "Lambda"),
    ('Ξ', "Xi"),
    ('Π', "Pi"),
    ('Σ', "Sigma"),
    ('Φ', "Phi"),
    ('Ψ', "Psi"),
    ('Ω', "Omega"),
    // Calculus / operators
    ('∑', "sum sigma"),
    ('∏', "product pi"),
    ('∫', "integral"),
    ('∬', "double integral"),
    ('∭', "triple integral"),
    ('∮', "contour integral"),
    ('∂', "partial derivative"),
    ('∇', "nabla del gradient"),
    ('√', "sqrt square root"),
    ('∛', "cube root"),
    ('∜', "fourth root"),
    ('∞', "infinity infty"),
    ('ℏ', "hbar planck"),
    ('∠', "angle"),
    // Arithmetic / relations
    ('±', "plus minus pm"),
    ('∓', "minus plus mp"),
    ('×', "times cross multiply"),
    ('÷', "div division"),
    ('·', "cdot dot multiply"),
    ('°', "degree"),
    ('′', "prime"),
    ('″', "double prime"),
    // Comparators
    ('≤', "leq less than or equal"),
    ('≥', "geq greater than or equal"),
    ('≠', "neq not equal"),
    ('≈', "approx approximately equal"),
    ('≡', "equiv identical congruent"),
    ('≢', "not equiv"),
    ('≪', "much less than"),
    ('≫', "much greater than"),
    ('≺', "prec precedes"),
    ('≻', "succ succeeds"),
    ('∝', "propto proportional"),
    ('∼', "sim similar"),
    // Set notation
    ('∈', "in element of"),
    ('∉', "notin not element"),
    ('∋', "ni contains"),
    ('∅', "emptyset empty"),
    ('∪', "cup union"),
    ('∩', "cap intersection"),
    ('⊂', "subset"),
    ('⊃', "superset"),
    ('⊆', "subseteq subset equal"),
    ('⊇', "supseteq superset equal"),
    ('ℕ', "naturals nat"),
    ('ℤ', "integers"),
    ('ℚ', "rationals"),
    ('ℝ', "reals"),
    ('ℂ', "complex"),
    // Logic
    ('∀', "forall for all"),
    ('∃', "exists there exists"),
    ('∄', "nexists not exists"),
    ('∧', "and wedge"),
    ('∨', "or vee"),
    ('¬', "not negation"),
    ('⊕', "xor oplus"),
    ('⊗', "otimes tensor"),
    ('⊙', "odot"),
    ('⊥', "perp perpendicular bottom"),
    ('⊤', "top"),
    ('⟹', "implies rightarrow double"),
    ('⟺', "iff if and only if biconditional"),
    // Arrows
    ('→', "to rightarrow"),
    ('←', "leftarrow"),
    ('↑', "uparrow"),
    ('↓', "downarrow"),
    ('↔', "leftrightarrow"),
    ('⇒', "Rightarrow"),
    ('⇐', "Leftarrow"),
    ('⇑', "Uparrow"),
    ('⇓', "Downarrow"),
    ('⇔', "Leftrightarrow"),
    ('↦', "mapsto"),
    // Fractions / superscripts
    ('½', "half one half fraction"),
    ('⅓', "third one third fraction"),
    ('¼', "quarter one quarter fraction"),
    ('¾', "three quarters fraction"),
    ('²', "squared superscript 2"),
    ('³', "cubed superscript 3"),
    // Brackets / delimiters
    ('⌈', "lceil ceiling"),
    ('⌉', "rceil ceiling"),
    ('⌊', "lfloor floor"),
    ('⌋', "rfloor floor"),
    ('⟨', "langle left angle bracket"),
    ('⟩', "rangle right angle bracket"),
];

/// Return the subset of [`SYMBOLS`] whose name contains `query` (case-insensitive),
/// or all symbols when `query` is empty.
pub fn filter_symbols(query: &str) -> Vec<(char, &'static str)> {
    let q = query.trim().to_ascii_lowercase();
    SYMBOLS
        .iter()
        .filter(|(ch, name)| {
            if q.is_empty() {
                return true;
            }
            name.to_ascii_lowercase().contains(&q) || ch.to_string() == q
        })
        .copied()
        .collect()
}

pub struct App {
    pub cfg: Config,
    pub theme: ResolvedTheme,
    store: Store,

    pub notes: Vec<Note>,
    pub folders: Vec<Folder>,
    /// The folder currently being viewed, or `None` for the top level.
    pub current_folder: Option<String>,
    /// What the grid currently shows (folders + notes), after folder/search scoping.
    pub items: Vec<ListItem>,
    pub selected: usize,
    pub list_columns: usize,

    pub screen: Screen,
    pub focus: Focus,
    pub search_query: String,

    /// Whether the left notes/folders drawer is shown (over the list or editor).
    pub drawer_open: bool,
    /// Folder ids currently expanded in the drawer tree.
    pub expanded: HashSet<String>,
    /// Cursor row within the drawer tree.
    pub drawer_selected: usize,
    /// First visible drawer row; scrolls independently of the main screen.
    pub drawer_scroll: usize,

    pub editor_view: EditorView,
    pub textarea: TextArea<'static>,
    pub editing: Option<Note>,
    editing_is_new: bool,

    pub modal: Modal,
    pub status: String,
    status_clear_at: Option<Instant>,

    refine_rx: Option<Receiver<RefineMsg>>,
    refine_status: RefineStatus,
    spinner_frame: usize,

    /// The local model server whose lifetime we own. In load-at-startup mode this
    /// is set once at launch; in on-demand mode it is started on the first refine
    /// and torn down again after `server_idle_deadline`. `None` when no server is
    /// managed (auto-start off, an unmanaged server already serves the port, or
    /// on-demand and currently shut down).
    server: Option<ManagedServer>,
    /// On-demand mode: when to shut the managed server down for being idle. Set
    /// after each refine completes; cleared when another refine reuses the server.
    server_idle_deadline: Option<Instant>,
    /// Error from a failed eager server start, deferred from launch so the app
    /// opens cleanly. Surfaced as a modal when the user first tries to refine.
    server_start_error: Option<String>,

    /// Speech-to-text dictation. The worker thread is spawned lazily on first use.
    gesture: GestureState,
    dictation_tx: Option<Sender<DictationCmd>>,
    dictation_rx: Option<Receiver<DictationMsg>>,
    dictation_handle: Option<JoinHandle<()>>,
    dictation_status: DictationStatus,
    /// True while the model is loading, so the "ready" flash fires once on
    /// Loading → Ready (not after every dictation).
    dictation_loading: bool,
    /// Smoothed mic input level in [0, 1] while capturing. Fast attack, slow
    /// release (an envelope follower) so the listening cursor reacts instantly to
    /// speech but falls back smoothly during pauses.
    dictation_level: f32,

    dirty: bool,
    last_edit: Option<Instant>,

    pub should_quit: bool,
}

impl App {
    pub fn new(cfg: Config, store: Store) -> Self {
        let notes = store.list().unwrap_or_default();
        let folders = store.list_folders().unwrap_or_default();
        let theme = ResolvedTheme::from_config(&cfg.theme);
        // Release events are unknown until the terminal is set up; default to the
        // fallback (no release) and let main flip it via `set_release_supported`.
        let gesture = GestureState::new(config::parse_key_binding(&cfg.dictation_key), false);
        let mut app = App {
            cfg,
            theme,
            store,
            notes,
            folders,
            current_folder: None,
            items: Vec::new(),
            selected: 0,
            list_columns: 1,
            screen: Screen::List,
            focus: Focus::Tiles,
            search_query: String::new(),
            drawer_open: false,
            expanded: HashSet::new(),
            drawer_selected: 0,
            drawer_scroll: 0,
            editor_view: EditorView::Original,
            textarea: TextArea::default(),
            editing: None,
            editing_is_new: false,
            modal: Modal::None,
            status: String::new(),
            status_clear_at: None,
            refine_rx: None,
            refine_status: RefineStatus::Idle,
            spinner_frame: 0,
            server: None,
            server_idle_deadline: None,
            server_start_error: None,
            gesture,
            dictation_tx: None,
            dictation_rx: None,
            dictation_handle: None,
            dictation_status: DictationStatus::Idle,
            dictation_loading: false,
            dictation_level: 0.0,
            dirty: false,
            last_edit: None,
            should_quit: false,
        };
        app.rebuild_items();
        app
    }

    /// Tell the dictation gesture recognizer whether the terminal reports key
    /// release events (Kitty keyboard protocol). With releases, hold-to-talk is
    /// exact; without, it falls back to an auto-repeat/timeout heuristic.
    pub fn set_release_supported(&mut self, supported: bool) {
        self.gesture.set_release_supported(supported);
    }

    /// Current dictation status, shown in the editor footer.
    pub fn dictation_status(&self) -> DictationStatus {
        self.dictation_status
    }

    /// Smoothed mic input level in [0, 1], for the animated listening cursor.
    pub fn dictation_level(&self) -> f32 {
        self.dictation_level
    }

    /// Whether dictation is actively capturing or transcribing (drives the
    /// recording indicator + its spinner animation).
    pub fn dictation_active(&self) -> bool {
        matches!(
            self.dictation_status,
            DictationStatus::Listening | DictationStatus::Live | DictationStatus::Transcribing
        )
    }


    // ---- managed model server ---------------------------------------------

    /// Start the local model server at launch, unless on-demand mode is selected
    /// (in which case it is deferred to the first refine). A failure is stored and
    /// shown when the user first tries to refine, so the app always opens cleanly.
    pub fn start_managed_server_if_eager(&mut self) {
        if self.cfg.server_on_demand {
            return; // lazy: started on the first Ctrl+R, see `ensure_server_started`
        }
        match ManagedServer::start(&self.cfg) {
            Ok(Some(server)) => {
                self.server = Some(server);
                self.set_status(format!(
                    "{} server starting — the model may take a moment to load.",
                    self.cfg.provider.label()
                ));
            }
            Ok(None) => {} // auto-start off, or an unmanaged server already serves
            Err(e) => {
                self.server_start_error = Some(format!(
                    "{} server did not start:\n\n{e:#}",
                    self.cfg.provider.label()
                ));
            }
        }
    }

    /// Stop the managed server synchronously on quit. A no-op when none is running.
    /// Kept synchronous (and printing after the terminal is restored) so the server
    /// is signalled before the process exits.
    pub fn stop_managed_server(&mut self) {
        if let Some(server) = self.server.take() {
            println!("Stopping {} server…", self.cfg.provider.label());
            drop(server); // ManagedServer::drop sends SIGTERM/SIGKILL and clears the pid file
        }
        self.server_idle_deadline = None;
    }

    /// Ensure a managed server is running before an on-demand refine, starting one
    /// lazily if needed. No-op in load-at-startup mode (the server is already up).
    /// Returns an error only if launching the server failed.
    fn ensure_server_started(&mut self) -> Result<()> {
        if !self.cfg.server_on_demand {
            return Ok(());
        }
        // A refine is about to use the server, so cancel any pending idle shutdown.
        self.server_idle_deadline = None;
        if self.server.is_some() {
            return Ok(()); // already warm from a recent refine
        }
        if let Some(server) = ManagedServer::start(&self.cfg)? {
            self.server = Some(server);
            self.set_status(format!(
                "Starting {} server — loading the model may take a moment…",
                self.cfg.provider.label()
            ));
        }
        // `None` => auto-start off or an unmanaged server already serves the port;
        // either way the refine proceeds against whatever `base_url` points to.
        Ok(())
    }

    /// In on-demand mode, schedule (or perform) shutdown of the managed server now
    /// that a refine has finished. With a zero idle timeout it stops right away;
    /// otherwise it stays warm until `server_idle_timeout_secs` of inactivity.
    fn schedule_server_idle_shutdown(&mut self) {
        if !self.cfg.server_on_demand || self.server.is_none() {
            return;
        }
        if self.cfg.server_idle_timeout_secs == 0 {
            self.shutdown_server_async();
        } else {
            self.server_idle_deadline =
                Some(Instant::now() + Duration::from_secs(self.cfg.server_idle_timeout_secs));
        }
    }

    /// Tear down the managed server off the UI thread, so the SIGTERM/SIGKILL wait
    /// never stalls rendering. Safe to abandon: a server left half-stopped is
    /// re-adopted and cleaned up via the pid file on the next launch.
    fn shutdown_server_async(&mut self) {
        self.server_idle_deadline = None;
        if let Some(server) = self.server.take() {
            std::thread::spawn(move || drop(server));
        }
    }

    /// Set the transient status-bar message (auto-clears after 3 seconds).
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_clear_at = Some(Instant::now() + Duration::from_secs(3));
    }

    // ---- periodic tick -----------------------------------------------------

    pub fn on_tick(&mut self) {
        self.poll_refine();
        // Ease the input-level meter down each tick (the "release" of the
        // envelope); fresh peaks drained in `poll_dictation` below pull it back up.
        if self.dictation_level > 0.0 {
            self.dictation_level *= LEVEL_RELEASE;
            if self.dictation_level < 0.01 {
                self.dictation_level = 0.0;
            }
        }
        self.poll_dictation();
        // Time-based gesture transitions (start push-to-talk after the hold
        // threshold; synthesize a release when auto-repeats stop in fallback mode).
        let action = self.gesture.on_tick(Instant::now());
        self.apply_gesture(action);
        if self.refine_status == RefineStatus::Refining || self.dictation_active() {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
        }
        // Auto-clear transient status message.
        if let Some(at) = self.status_clear_at {
            if Instant::now() >= at {
                self.status.clear();
                self.status_clear_at = None;
            }
        }
        // On-demand mode: shut the model server down once it has been idle long
        // enough, freeing its memory until the next refine.
        if let Some(deadline) = self.server_idle_deadline {
            if !self.is_refining() && Instant::now() >= deadline {
                self.shutdown_server_async();
                self.set_status("Stopped the model server to free memory.");
            }
        }
        // Debounced autosave while editing.
        if self.screen == Screen::Editor && self.dirty {
            if let Some(t) = self.last_edit {
                if t.elapsed() >= AUTOSAVE_DEBOUNCE {
                    self.autosave();
                }
            }
        }
    }

    pub fn refine_status(&self) -> RefineStatus {
        self.refine_status
    }

    fn is_refining(&self) -> bool {
        self.refine_status == RefineStatus::Refining
    }

    pub fn spinner(&self) -> char {
        const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        FRAMES[self.spinner_frame % FRAMES.len()]
    }

    fn poll_refine(&mut self) {
        let msg = match &self.refine_rx {
            Some(rx) => match rx.try_recv() {
                Ok(msg) => msg,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.refine_rx = None;
                    self.refine_status = RefineStatus::Idle;
                    self.schedule_server_idle_shutdown();
                    return;
                }
            },
            None => return,
        };
        self.refine_rx = None;
        match msg {
            RefineMsg::Done(text) => {
                let mut save_err = None;
                if let Some(note) = self.editing.as_mut() {
                    note.refined = Some(text.clone());
                    // Auto-title from the refined output unless the user set a title.
                    if !note.meta.title_custom {
                        if let Some(t) = note::derive_title(&text) {
                            note.meta.title = t;
                        }
                    }
                    if let Err(e) = self.store.save(note) {
                        save_err = Some(e);
                    }
                }
                self.editor_view = EditorView::Refined;
                self.load_view();
                self.dirty = false;
                match save_err {
                    Some(e) => {
                        self.refine_status = RefineStatus::Error;
                        self.modal = Modal::Error(format!("Refined, but failed to save:\n\n{e:#}"));
                    }
                    None => self.refine_status = RefineStatus::Done,
                }
            }
            RefineMsg::Error(e) => {
                self.refine_status = RefineStatus::Error;
                self.modal = Modal::Error(format!("Refine failed:\n\n{e}"));
            }
        }
        // The refine is done (success or failure); in on-demand mode start the
        // idle countdown that eventually frees the server's memory.
        self.schedule_server_idle_shutdown();
    }

    fn autosave(&mut self) {
        if !self.dirty {
            return;
        }
        if let Err(e) = self.save_editing() {
            self.set_status(format!("Autosave failed: {e}"));
        }
    }

    // ---- note list helpers -------------------------------------------------

    /// Reload notes and folders from disk, then rebuild the visible grid.
    fn refresh(&mut self) {
        match self.store.list() {
            Ok(n) => self.notes = n,
            Err(e) => {
                self.set_status(format!("Failed to load notes: {e}"));
                self.notes = Vec::new();
            }
        }
        match self.store.list_folders() {
            Ok(f) => self.folders = f,
            Err(e) => self.set_status(format!("Failed to load folders: {e}")),
        }
        // A folder we were viewing may have been removed externally.
        if let Some(id) = &self.current_folder {
            if !self.folders.iter().any(|f| &f.id == id) {
                self.current_folder = None;
            }
        }
        self.rebuild_items();
    }

    /// Recompute [`App::items`] for the current folder and search query.
    /// Top level shows folder tiles followed by loose notes; an active search
    /// flattens to matching notes (across all folders at the top level, or within
    /// the open folder). Notes whose folder id no longer exists fall back to the
    /// top level so they can never become unreachable.
    fn rebuild_items(&mut self) {
        let valid: HashSet<&str> = self.folders.iter().map(|f| f.id.as_str()).collect();
        let current = self.current_folder.as_deref();
        let in_container = |n: &Note| match (current, n.meta.folder.as_deref()) {
            (Some(f), nf) => nf == Some(f),
            (None, Some(fid)) => !valid.contains(fid), // orphaned note => top level
            (None, None) => true,
        };

        let mut items = Vec::new();
        let query = self.search_query.trim();
        if query.is_empty() {
            if current.is_none() {
                // Merge folders and loose notes into a single list sorted by
                // effective modified time (newest first). A folder's effective
                // modified time is the most recent note modification inside it,
                // falling back to its creation time when empty.
                let mut entries = Vec::new();
                for (fi, folder) in self.folders.iter().enumerate() {
                    let t = self
                        .notes
                        .iter()
                        .filter(|n| n.meta.folder.as_deref() == Some(folder.id.as_str()))
                        .map(|n| n.meta.modified)
                        .max()
                        .unwrap_or(folder.created);
                    entries.push((t, ListItem::Folder(fi)));
                }
                for (ni, n) in self.notes.iter().enumerate() {
                    if in_container(n) {
                        entries.push((n.meta.modified, ListItem::Note(ni)));
                    }
                }
                entries.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
                items.extend(entries.into_iter().map(|(_, item)| item));
            } else {
                for (i, n) in self.notes.iter().enumerate() {
                    if in_container(n) {
                        items.push(ListItem::Note(i));
                    }
                }
            }
        } else {
            // Best-match-first across all notes; keep only those in scope. At the
            // top level the scope is every note, so notes inside folders stay findable.
            for i in search::filter_indices(&self.notes, query) {
                let n = &self.notes[i];
                let keep = match current {
                    Some(f) => n.meta.folder.as_deref() == Some(f),
                    None => true,
                };
                if keep {
                    items.push(ListItem::Note(i));
                }
            }
        }

        self.items = items;
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
    }

    fn selected_note(&self) -> Option<&Note> {
        match self.items.get(self.selected) {
            Some(ListItem::Note(i)) => self.notes.get(*i),
            _ => None,
        }
    }

    fn selected_folder(&self) -> Option<&Folder> {
        match self.items.get(self.selected) {
            Some(ListItem::Folder(i)) => self.folders.get(*i),
            _ => None,
        }
    }

    /// Notes filed in `folder_id`, newest-first (mirrors the note ordering).
    pub fn folder_notes(&self, folder_id: &str) -> Vec<&Note> {
        self.notes
            .iter()
            .filter(|n| n.meta.folder.as_deref() == Some(folder_id))
            .collect()
    }

    fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len() as isize - 1;
        let next = (self.selected as isize + delta).clamp(0, max);
        self.selected = next as usize;
    }

    // ---- editor helpers ----------------------------------------------------

    fn build_textarea(&mut self, content: &str) {
        let lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            content.split('\n').map(|s| s.to_string()).collect()
        };
        // The editor body (block, border, word-wrap, cursor) is rendered in
        // `ui::editor`; the TextArea is only the text/cursor model here.
        let mut ta = TextArea::new(lines);
        ta.set_cursor_line_style(Style::default());
        self.textarea = ta;
    }

    /// The text currently in the editor.
    fn active_content(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Persist the editor's text back into the note model for the active view.
    fn sync_active(&mut self) {
        let content = self.active_content();
        if let Some(note) = self.editing.as_mut() {
            match self.editor_view {
                EditorView::Original => note.content = content,
                EditorView::Refined => note.refined = Some(content),
            }
        }
    }

    /// Load the note's content for the active view into the editor textarea.
    fn load_view(&mut self) {
        let content = match (&self.editing, self.editor_view) {
            (Some(n), EditorView::Original) => n.content.clone(),
            (Some(n), EditorView::Refined) => n.refined.clone().unwrap_or_default(),
            (None, _) => String::new(),
        };
        self.build_textarea(&content);
    }

    fn reset_refine_state(&mut self) {
        self.refine_rx = None;
        self.refine_status = RefineStatus::Idle;
    }

    fn new_note(&mut self) {
        let mut note = Note::new();
        // A note created while viewing a folder is filed into that folder.
        note.meta.folder = self.current_folder.clone();
        self.editing = Some(note);
        self.editing_is_new = true;
        self.editor_view = EditorView::Original;
        self.load_view();
        self.screen = Screen::Editor;
        self.reset_refine_state();
        self.warmup_dictation();
        self.set_status("New note — start typing");
    }

    fn open_selected(&mut self) {
        if let Some(note) = self.selected_note().cloned() {
            self.open_note(note);
        }
    }

    /// Open `note` in the editor, defaulting to its refined view when one exists.
    fn open_note(&mut self, note: Note) {
        self.editor_view = if note.refined.is_some() {
            EditorView::Refined
        } else {
            EditorView::Original
        };
        self.editing = Some(note);
        self.editing_is_new = false;
        self.load_view();
        self.screen = Screen::Editor;
        self.reset_refine_state();
        self.warmup_dictation();
        self.status.clear();
    }

    fn save_editing(&mut self) -> Result<()> {
        self.sync_active();
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        if let Some(note) = self.editing.as_mut() {
            if self.editing_is_new && note.content.trim().is_empty() && note.refined.is_none() {
                return Ok(()); // discard an untouched new note
            }
            self.store.save(note)?;
            self.editing_is_new = false;
        }
        Ok(())
    }

    fn leave_editor(&mut self) {
        let result = self.save_editing();
        self.stop_dictation();
        self.editing = None;
        self.editor_view = EditorView::Original;
        self.reset_refine_state();
        // Opening a note from a search dismisses that search in the background:
        // returning to the list shows the full, unfiltered set with no query left
        // active. `refresh` rebuilds the grid below, so clear before it runs.
        self.search_query.clear();
        if self.focus == Focus::Search {
            self.focus = Focus::Tiles;
        }
        self.refresh();
        self.screen = Screen::List;
        if let Err(e) = result {
            self.modal = Modal::Error(format!("Failed to save:\n\n{e:#}"));
        }
    }

    fn toggle_view(&mut self) {
        let has_refined = self
            .editing
            .as_ref()
            .map(|n| n.refined.is_some())
            .unwrap_or(false);
        if !has_refined {
            self.set_status("No refined version yet — press Ctrl+R");
            return;
        }
        self.sync_active();
        self.editor_view = match self.editor_view {
            EditorView::Original => EditorView::Refined,
            EditorView::Refined => EditorView::Original,
        };
        self.load_view();
    }

    fn start_refine(&mut self) {
        let prompt = self.cfg.refine_prompt.clone();
        self.start_refine_impl(prompt);
    }

    fn start_refine_custom(&mut self, prompt: String) {
        self.start_refine_impl(prompt);
    }

    fn start_refine_impl(&mut self, prompt: String) {
        if self.is_refining() || self.editing.is_none() {
            return;
        }
        // If the managed server failed to start at launch, surface that error now
        // rather than sending a request that is guaranteed to fail.
        if let Some(err) = self.server_start_error.clone() {
            self.modal = Modal::Error(err);
            return;
        }
        // Refine whatever is currently in the editor (original, or an already-refined
        // draft being iterated on). The result always becomes the refined version.
        self.sync_active();
        let source = self.active_content();
        if source.trim().is_empty() {
            self.set_status("Nothing to refine yet");
            return;
        }
        // On-demand mode: spin the server up now (no-op when it is already running
        // or managed eagerly). A launch failure aborts the refine with an error.
        if let Err(e) = self.ensure_server_started() {
            self.modal = Modal::Error(format!(
                "Couldn't start the {} server:\n\n{e:#}",
                self.cfg.provider.label()
            ));
            return;
        }
        // When we manage the server lazily, the model may still be loading, so wait
        // for its port to come up before sending. An already-running server (eager
        // mode, or one we don't manage) fails fast with no wait.
        let wait_ready = if self.cfg.server_on_demand && self.cfg.auto_start_server {
            Duration::from_secs(self.cfg.request_timeout_secs)
        } else {
            Duration::ZERO
        };
        self.refine_rx = Some(llm::spawn_refine(&self.cfg, prompt, source, wait_ready));
        self.refine_status = RefineStatus::Refining;
        self.spinner_frame = 0;
        // Clear any stale status (eager mode behaves as before). In on-demand mode
        // we keep the "loading the model…" hint ensure_server_started may have set.
        if !self.cfg.server_on_demand {
            self.status.clear();
        }
    }

    fn delete_selected(&mut self) {
        if let Some(note) = self.selected_note() {
            let id = note.meta.id.clone();
            let _ = self.store.delete(&id);
        }
        self.refresh();
        self.set_status("Note deleted");
    }

    // ---- folders -----------------------------------------------------------

    /// Open the selected tile: enter a folder, or open a note in the editor.
    fn open_or_enter(&mut self) {
        match self.items.get(self.selected) {
            Some(ListItem::Folder(i)) => {
                let id = self.folders[*i].id.clone();
                self.enter_folder(id);
            }
            Some(ListItem::Note(_)) => self.open_selected(),
            None => {}
        }
    }

    fn enter_folder(&mut self, id: String) {
        self.current_folder = Some(id);
        self.selected = 0;
        self.search_query.clear();
        self.focus = Focus::Tiles;
        self.rebuild_items();
    }

    /// Return from a folder view to the top level.
    fn leave_folder(&mut self) {
        self.current_folder = None;
        self.selected = 0;
        self.search_query.clear();
        self.rebuild_items();
    }

    /// Dismiss the search: drop the query and any filtering, and return the
    /// keyboard to the tiles. Safe to call when no search is active.
    fn exit_search(&mut self) {
        self.search_query.clear();
        self.rebuild_items();
        self.focus = Focus::Tiles;
    }

    fn create_folder(&mut self, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            self.set_status("Folder needs a name");
            return;
        }
        self.folders.push(Folder::new(title.to_string()));
        if let Err(e) = self.store.save_folders(&self.folders) {
            self.modal = Modal::Error(format!("Failed to save folder:\n\n{e:#}"));
            return;
        }
        self.rebuild_items();
        self.set_status(format!("Folder “{title}” created"));
    }

    /// File the note with `note_id` into `target` (`None` => top level) and persist.
    fn move_note_to(&mut self, note_id: &str, target: Option<String>) {
        let Some(note) = self.notes.iter().find(|n| n.meta.id == note_id) else {
            return;
        };
        let mut note = note.clone();
        note.meta.folder = target.clone();
        if let Err(e) = self.store.save(&mut note) {
            self.modal = Modal::Error(format!("Failed to move note:\n\n{e:#}"));
            return;
        }
        self.refresh();
        let where_to = match target.and_then(|id| self.folder_title(&id)) {
            Some(t) => format!("“{t}”"),
            None => "the top level".to_string(),
        };
        self.set_status(format!("Moved note to {where_to}"));
    }

    /// Delete a folder and return its notes to the top level.
    fn delete_folder(&mut self, id: &str) {
        let note_ids: Vec<String> = self
            .notes
            .iter()
            .filter(|n| n.meta.folder.as_deref() == Some(id))
            .map(|n| n.meta.id.clone())
            .collect();
        for nid in note_ids {
            if let Some(note) = self.notes.iter().find(|n| n.meta.id == nid) {
                let mut note = note.clone();
                note.meta.folder = None;
                let _ = self.store.save(&mut note);
            }
        }
        self.folders.retain(|f| f.id != id);
        if let Err(e) = self.store.save_folders(&self.folders) {
            self.modal = Modal::Error(format!("Failed to delete folder:\n\n{e:#}"));
            return;
        }
        if self.current_folder.as_deref() == Some(id) {
            self.current_folder = None;
        }
        self.refresh();
        self.set_status("Folder deleted — its notes moved to the top level");
    }

    fn folder_title(&self, id: &str) -> Option<String> {
        self.folders
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.title.clone())
    }

    // ---- drawer (notes/folders tree) ---------------------------------------

    /// Flatten the folders + notes into the rows the drawer renders. Expanded
    /// folders are followed by their notes (indented); loose notes (no folder, or
    /// a folder id that no longer exists) come last at the top level.
    pub fn drawer_rows(&self) -> Vec<DrawerRow> {
        let valid: HashSet<&str> = self.folders.iter().map(|f| f.id.as_str()).collect();

        // Sort folder indices by effective modified time (newest first), matching
        // the order used in the home screen grid.
        let mut folder_order: Vec<usize> = (0..self.folders.len()).collect();
        folder_order.sort_by_key(|&fi| {
            let folder = &self.folders[fi];
            let t = self
                .notes
                .iter()
                .filter(|n| n.meta.folder.as_deref() == Some(folder.id.as_str()))
                .map(|n| n.meta.modified)
                .max()
                .unwrap_or(folder.created);
            std::cmp::Reverse(t)
        });

        let mut rows = Vec::new();
        for fi in folder_order {
            let folder = &self.folders[fi];
            let expanded = self.expanded.contains(&folder.id);
            rows.push(DrawerRow::Folder {
                index: fi,
                expanded,
                count: self.folder_notes(&folder.id).len(),
            });
            if expanded {
                for (ni, n) in self.notes.iter().enumerate() {
                    if n.meta.folder.as_deref() == Some(folder.id.as_str()) {
                        rows.push(DrawerRow::Note {
                            index: ni,
                            child: true,
                        });
                    }
                }
            }
        }
        for (ni, n) in self.notes.iter().enumerate() {
            let loose = match n.meta.folder.as_deref() {
                Some(fid) => !valid.contains(fid),
                None => true,
            };
            if loose {
                rows.push(DrawerRow::Note {
                    index: ni,
                    child: false,
                });
            }
        }
        rows
    }

    /// Toggle the drawer. Opening it (only ever from the list screen) also gives
    /// it the keyboard focus.
    fn toggle_drawer(&mut self) {
        if self.drawer_open {
            self.close_drawer();
        } else {
            self.drawer_open = true;
            self.focus = Focus::Drawer;
            let rows = self.drawer_rows().len();
            if self.drawer_selected >= rows {
                self.drawer_selected = rows.saturating_sub(1);
            }
        }
    }

    fn close_drawer(&mut self) {
        self.drawer_open = false;
        if self.focus == Focus::Drawer {
            self.focus = Focus::Tiles;
        }
    }

    fn on_key_drawer(&mut self, key: KeyEvent) {
        let rows = self.drawer_rows();
        if rows.is_empty() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('h')) {
                self.close_drawer();
            }
            return;
        }
        let max = rows.len() - 1;
        self.drawer_selected = self.drawer_selected.min(max);
        match key.code {
            // `h` is the toggle: in the drawer it always closes (Esc too).
            KeyCode::Char('h') | KeyCode::Esc => self.close_drawer(),
            KeyCode::Tab => self.focus = Focus::Tiles, // keep the drawer open
            KeyCode::Char('q') => self.modal = Modal::ConfirmQuit,
            // Navigation is arrow-keys only.
            KeyCode::Down => {
                self.drawer_selected = (self.drawer_selected + 1).min(max);
            }
            KeyCode::Up => {
                self.drawer_selected = self.drawer_selected.saturating_sub(1);
            }
            KeyCode::Right => {
                if let Some(DrawerRow::Folder {
                    index,
                    expanded: false,
                    ..
                }) = rows.get(self.drawer_selected)
                {
                    self.expanded.insert(self.folders[*index].id.clone());
                }
            }
            // Left collapses the folder / steps a nested note up to its parent.
            KeyCode::Left => {
                self.drawer_collapse_or_parent(&rows);
            }
            KeyCode::Enter => match rows.get(self.drawer_selected) {
                Some(DrawerRow::Folder {
                    index, expanded, ..
                }) => {
                    let id = self.folders[*index].id.clone();
                    if *expanded {
                        self.expanded.remove(&id);
                    } else {
                        self.expanded.insert(id);
                    }
                }
                Some(DrawerRow::Note { index, .. }) => {
                    let note = self.notes[*index].clone();
                    self.open_note(note);
                }
                None => {}
            },
            _ => {}
        }
    }

    /// Collapse the selected folder, or step a nested note up to its parent.
    /// Returns whether it did anything (false on a leaf with nowhere to go).
    fn drawer_collapse_or_parent(&mut self, rows: &[DrawerRow]) -> bool {
        match rows.get(self.drawer_selected) {
            Some(DrawerRow::Folder {
                index,
                expanded: true,
                ..
            }) => {
                let id = self.folders[*index].id.clone();
                self.expanded.remove(&id);
                true
            }
            Some(DrawerRow::Note { child: true, .. }) => {
                if let Some(pos) = rows[..self.drawer_selected]
                    .iter()
                    .rposition(|r| matches!(r, DrawerRow::Folder { .. }))
                {
                    self.drawer_selected = pos;
                }
                true
            }
            _ => false,
        }
    }

    fn open_export_modal(&mut self) {
        let title = if self.screen == Screen::Editor {
            self.editing.as_ref().map(|n| n.meta.title.clone())
        } else {
            self.selected_note().map(|n| n.meta.title.clone())
        };
        let Some(title) = title else {
            self.set_status("Nothing to export");
            return;
        };
        let slug = crate::note::slugify(&title);
        let dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let default = dir.join(format!("{slug}.md"));
        self.modal = Modal::Export(default.to_string_lossy().into_owned());
    }

    fn do_export(&mut self, path: &str) {
        let dest = config::expand_tilde(path.trim());
        let note = if self.screen == Screen::Editor {
            self.editing.clone()
        } else {
            self.selected_note().cloned()
        };
        let Some(note) = note else {
            self.modal = Modal::Error("No note to export.".into());
            return;
        };
        let use_refined = self.screen == Screen::Editor && self.editor_view == EditorView::Refined;
        match storage::export(&note, use_refined, &dest) {
            Ok(()) => self.modal = Modal::Info(format!("Exported to:\n\n{}", dest.display())),
            Err(e) => self.modal = Modal::Error(format!("Export failed:\n\n{e:#}")),
        }
    }

    // ---- key handling ------------------------------------------------------

    /// Raw entry point for every key event from the loop (Press/Repeat/Release).
    /// The dictation key (in the editor, no modal open) drives the gesture
    /// recognizer; all other keys go through the normal dispatch on Press — and on
    /// Repeat too, so auto-repeat typing still works when the Kitty protocol turns
    /// held keys into Press+Repeat instead of repeated Press events.
    pub fn on_key_event(&mut self, key: KeyEvent) {
        // Receiving any real release event proves the terminal supports the Kitty
        // keyboard protocol, regardless of what the startup capability query said.
        if key.kind == KeyEventKind::Release {
            self.gesture.set_release_supported(true);
        }
        if self.screen == Screen::Editor
            && matches!(self.modal, Modal::None)
            && self.gesture.matches(&key)
        {
            let action = self.gesture.on_key(key.kind, Instant::now());
            self.apply_gesture(action);
            return;
        }
        if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            self.on_key(key);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if !matches!(self.modal, Modal::None) {
            self.on_key_modal(key);
            return;
        }
        match self.screen {
            Screen::List => self.on_key_list(key),
            Screen::Editor => self.on_key_editor(key),
        }
    }

    // ---- dictation ---------------------------------------------------------

    /// Spawn the dictation worker on first use (lazy, so the model isn't loaded
    /// unless dictation is actually triggered).
    fn ensure_dictation(&mut self) {
        if self.dictation_tx.is_none() {
            let (tx, rx, handle) = dictation::spawn(&self.cfg);
            self.dictation_tx = Some(tx);
            self.dictation_rx = Some(rx);
            self.dictation_handle = Some(handle);
        }
    }

    fn apply_gesture(&mut self, action: Option<GestureAction>) {
        let Some(action) = action else { return };
        self.ensure_dictation();
        let cmd = match action {
            GestureAction::StartPushToTalk => DictationCmd::StartPushToTalk,
            GestureAction::StopPushToTalk => DictationCmd::StopPushToTalk,
            GestureAction::ToggleLive => DictationCmd::ToggleLive,
        };
        if let Some(tx) = &self.dictation_tx {
            let _ = tx.send(cmd);
        }
    }

    /// Stop any in-progress dictation (e.g. on leaving the editor or quitting).
    fn stop_dictation(&mut self) {
        if let Some(tx) = &self.dictation_tx {
            let _ = tx.send(DictationCmd::Stop);
        }
        self.gesture.reset();
        self.dictation_status = DictationStatus::Idle;
        self.dictation_level = 0.0;
    }

    /// Drain transcription results and insert them into the editor.
    fn poll_dictation(&mut self) {
        let mut msgs = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.dictation_rx {
            loop {
                match rx.try_recv() {
                    Ok(m) => msgs.push(m),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.dictation_rx = None;
            self.dictation_tx = None;
            // Thread already exited (it disconnected), so the join is instant.
            if let Some(h) = self.dictation_handle.take() {
                let _ = h.join();
            }
            self.dictation_status = DictationStatus::Idle;
        }
        for m in msgs {
            match m {
                DictationMsg::Text(t) => {
                    if self.screen == Screen::Editor {
                        self.insert_dictated(&t);
                    }
                }
                DictationMsg::Status(s) => {
                    // Flash a transient "ready" exactly once, on Loading → Ready.
                    match s {
                        DictationStatus::Loading => self.dictation_loading = true,
                        DictationStatus::Ready if self.dictation_loading => {
                            self.dictation_loading = false;
                            self.set_status("🎤 Dictation ready");
                        }
                        _ => self.dictation_loading = false,
                    }
                    // Capture stopped (not listening): collapse the level meter so
                    // a stale peak doesn't linger on the cursor.
                    if !matches!(s, DictationStatus::Listening | DictationStatus::Live) {
                        self.dictation_level = 0.0;
                    }
                    // Live mode ends on a single press, so tell the gesture
                    // recognizer when a live session is active.
                    self.gesture.set_live_active(s == DictationStatus::Live);
                    self.dictation_status = s;
                }
                DictationMsg::Level(l) => {
                    // Instant attack: jump up to a new peak, let `on_tick` decay it.
                    self.dictation_level = self.dictation_level.max(normalize_level(l));
                }
                DictationMsg::Error(e) => self.set_status(format!("Dictation: {e}")),
            }
        }
    }

    /// Spawn the dictation worker and prefetch the speech model at app startup:
    /// download it if it isn't on disk yet, then load it — all on the worker
    /// thread and silently, so the UI never blocks and shows no status. By the
    /// time the user reaches the editor and dictates, the model is ready.
    pub fn prefetch_dictation(&mut self) {
        self.ensure_dictation();
        if let Some(tx) = &self.dictation_tx {
            let _ = tx.send(DictationCmd::Prefetch);
        }
    }

    /// Signal the dictation worker to stop and wait for it to exit. Must be
    /// called before process exit when the model has been loaded: whisper.cpp's
    /// Metal backend asserts during atexit cleanup if the WhisperContext is still
    /// alive when the C++ destructors run.
    pub fn join_dictation_thread(&mut self) {
        // Drop the sender first — this signals the worker to exit its run loop.
        self.dictation_tx = None;
        self.dictation_rx = None;
        if let Some(handle) = self.dictation_handle.take() {
            let _ = handle.join();
        }
    }

    /// Preload the speech model from disk on entering the editor, so the first
    /// dictation is instant. Loads only what's already present (startup's
    /// `prefetch_dictation` is what fetches it); a cheap no-op once it's loaded.
    fn warmup_dictation(&mut self) {
        self.ensure_dictation();
        if let Some(tx) = &self.dictation_tx {
            let _ = tx.send(DictationCmd::Warmup);
        }
    }

    /// Insert recognized speech at the cursor, joining onto existing text with a
    /// single space when needed. Marks the buffer dirty so autosave persists it.
    fn insert_dictated(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let needs_space = self
            .char_before_cursor()
            .map(|c| !c.is_whitespace())
            .unwrap_or(false);
        let payload = if needs_space {
            format!(" {text}")
        } else {
            text.to_string()
        };
        if self.textarea.insert_str(&payload) {
            self.dirty = true;
            self.last_edit = Some(Instant::now());
        }
    }

    /// The character immediately before the cursor, or `None` at the very start.
    fn char_before_cursor(&self) -> Option<char> {
        let (row, col) = self.textarea.cursor();
        if col == 0 {
            // Start of a line: the preceding char is a newline unless line 0.
            return if row == 0 { None } else { Some('\n') };
        }
        self.textarea
            .lines()
            .get(row)
            .and_then(|line| line.chars().nth(col - 1))
    }

    fn on_key_list(&mut self, key: KeyEvent) {
        // Ctrl+F creates a folder from anywhere in the list.
        if is_ctrl(&key, 'f') {
            self.modal = Modal::NewFolder(String::new());
            return;
        }
        match self.focus {
            Focus::Drawer => self.on_key_drawer(key),
            Focus::Search => match key.code {
                // Esc or `/` immediately dismisses search: clear the query and
                // its filtering, and hand the keyboard back to the tiles.
                KeyCode::Esc | KeyCode::Char('/') => self.exit_search(),
                // The highlighted result is already selected, so Enter opens it
                // straight from the search bar (returning later clears the search).
                KeyCode::Enter => self.open_or_enter(),
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.rebuild_items();
                }
                KeyCode::Down => self.move_selection(self.list_columns as isize),
                KeyCode::Up => self.move_selection(-(self.list_columns as isize)),
                KeyCode::Char(c) => {
                    self.search_query.push(c);
                    self.rebuild_items();
                }
                _ => {}
            },
            Focus::Tiles => match key.code {
                KeyCode::Char('q') => self.modal = Modal::ConfirmQuit,
                // Esc dismisses an active search first; otherwise it backs out of
                // a folder, and at the top level it prompts to quit.
                KeyCode::Esc => {
                    if !self.search_query.is_empty() {
                        self.exit_search();
                    } else if self.current_folder.is_some() {
                        self.leave_folder();
                    } else {
                        self.modal = Modal::ConfirmQuit;
                    }
                }
                // h toggles the notes/folders drawer (and focuses it).
                KeyCode::Char('h') => self.toggle_drawer(),
                // Tab moves into an already-open drawer without closing it.
                KeyCode::Tab if self.drawer_open => self.focus = Focus::Drawer,
                KeyCode::Char('n') => self.new_note(),
                KeyCode::Char('o') | KeyCode::Enter => self.open_or_enter(),
                KeyCode::Char('m') => self.open_move_modal(),
                KeyCode::Char('/') => self.focus = Focus::Search,
                KeyCode::Char('\\') if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.modal = Modal::Help;
                }
                KeyCode::Char('d') => {
                    if self.selected_note().is_some() {
                        self.modal = Modal::ConfirmDelete;
                    } else if let Some(folder) = self.selected_folder() {
                        self.modal = Modal::ConfirmDeleteFolder(folder.id.clone());
                    }
                }
                KeyCode::Char('x') => self.open_export_modal(),
                KeyCode::Char('j') | KeyCode::Down => {
                    self.move_selection(self.list_columns as isize)
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.move_selection(-(self.list_columns as isize))
                }
                // `h` now toggles the drawer (above); left movement is the arrow key.
                KeyCode::Left => self.move_selection(-1),
                KeyCode::Char('l') | KeyCode::Right => self.move_selection(1),
                _ => {}
            },
        }
    }

    /// Open the "move to folder" picker for the selected note, preselecting its
    /// current folder.
    fn open_move_modal(&mut self) {
        let Some(note) = self.selected_note() else {
            return;
        };
        let note_id = note.meta.id.clone();
        let sel = match note.meta.folder.as_deref() {
            Some(fid) => self
                .folders
                .iter()
                .position(|f| f.id == fid)
                .map(|i| i + 1)
                .unwrap_or(0),
            None => 0,
        };
        self.modal = Modal::MoveNote { note_id, sel };
    }

    fn on_key_editor(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.leave_editor();
            return;
        }
        if is_ctrl(&key, 's') {
            let r = self.save_editing();
            match r {
                Ok(()) => self.set_status("Saved ✓"),
                Err(e) => self.modal = Modal::Error(format!("{e:#}")),
            }
            return;
        }
        if is_ctrl(&key, 'r') {
            self.start_refine();
            return;
        }
        if is_ctrl(&key, 'p') {
            if !self.is_refining() {
                self.modal = Modal::CustomPrompt(String::new());
            }
            return;
        }
        if is_ctrl(&key, 't') {
            let title = self
                .editing
                .as_ref()
                .map(|n| n.meta.title.clone())
                .unwrap_or_default();
            let cursor = title.chars().count();
            self.modal = Modal::TitleEdit { buf: title, cursor };
            return;
        }
        if is_ctrl(&key, 'x') {
            self.open_export_modal();
            return;
        }
        if is_ctrl(&key, 'e') {
            self.modal = Modal::SymbolPicker {
                query: String::new(),
                sel: 0,
            };
            return;
        }
        if key.code == KeyCode::Tab {
            self.toggle_view();
            return;
        }
        if key.code == KeyCode::Char('\\') && key.modifiers.contains(KeyModifiers::ALT) {
            self.modal = Modal::Help;
            return;
        }

        // Both the original and refined views are editable. Track edits for autosave.
        if self.textarea.input(key) {
            self.dirty = true;
            self.last_edit = Some(Instant::now());
        }
    }

    fn on_key_modal(&mut self, key: KeyEvent) {
        // Take ownership so we can freely call &mut self methods inside the arms.
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::Help | Modal::Error(_) | Modal::Info(_) | Modal::None => {}
            Modal::ConfirmQuit => {
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) {
                    self.should_quit = true;
                }
            }
            Modal::ConfirmDelete => {
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) {
                    self.delete_selected();
                }
            }
            Modal::ConfirmDeleteFolder(id) => {
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) {
                    self.delete_folder(&id);
                }
            }
            Modal::NewFolder(mut buf) => match key.code {
                KeyCode::Enter => self.create_folder(&buf),
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    buf.pop();
                    self.modal = Modal::NewFolder(buf);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.modal = Modal::NewFolder(buf);
                }
                _ => self.modal = Modal::NewFolder(buf),
            },
            Modal::MoveNote { note_id, sel } => {
                // Rows: 0 = top level, then one per folder.
                let max = self.folders.len(); // last valid index
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        let sel = sel.saturating_sub(1);
                        self.modal = Modal::MoveNote { note_id, sel };
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let sel = (sel + 1).min(max);
                        self.modal = Modal::MoveNote { note_id, sel };
                    }
                    KeyCode::Enter => {
                        let target = if sel == 0 {
                            None
                        } else {
                            self.folders.get(sel - 1).map(|f| f.id.clone())
                        };
                        self.move_note_to(&note_id, target);
                    }
                    KeyCode::Esc => {}
                    _ => self.modal = Modal::MoveNote { note_id, sel },
                }
            }
            Modal::TitleEdit {
                mut buf,
                mut cursor,
            } => match key.code {
                KeyCode::Enter => {
                    let t = buf.trim().to_string();
                    if let Some(note) = self.editing.as_mut() {
                        if t.is_empty() {
                            note.meta.title = "Untitled Note".to_string();
                            note.meta.title_custom = false;
                        } else {
                            note.meta.title = t;
                            note.meta.title_custom = true;
                        }
                    }
                    self.dirty = true;
                    let _ = self.save_editing();
                    self.set_status("Title updated");
                }
                KeyCode::Esc => {}
                KeyCode::Left => {
                    cursor = cursor.saturating_sub(1);
                    self.modal = Modal::TitleEdit { buf, cursor };
                }
                KeyCode::Right => {
                    if cursor < buf.chars().count() {
                        cursor += 1;
                    }
                    self.modal = Modal::TitleEdit { buf, cursor };
                }
                KeyCode::Home => {
                    self.modal = Modal::TitleEdit { buf, cursor: 0 };
                }
                KeyCode::End => {
                    let cursor = buf.chars().count();
                    self.modal = Modal::TitleEdit { buf, cursor };
                }
                KeyCode::Backspace => {
                    if cursor > 0 {
                        let byte_idx = char_to_byte(&buf, cursor - 1);
                        buf.remove(byte_idx);
                        cursor -= 1;
                    }
                    self.modal = Modal::TitleEdit { buf, cursor };
                }
                KeyCode::Delete => {
                    if cursor < buf.chars().count() {
                        let byte_idx = char_to_byte(&buf, cursor);
                        buf.remove(byte_idx);
                    }
                    self.modal = Modal::TitleEdit { buf, cursor };
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let byte_idx = char_to_byte(&buf, cursor);
                    buf.insert(byte_idx, c);
                    cursor += 1;
                    self.modal = Modal::TitleEdit { buf, cursor };
                }
                _ => self.modal = Modal::TitleEdit { buf, cursor },
            },
            Modal::Export(mut buf) => match key.code {
                KeyCode::Enter => self.do_export(&buf),
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    buf.pop();
                    self.modal = Modal::Export(buf);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.modal = Modal::Export(buf);
                }
                _ => self.modal = Modal::Export(buf),
            },
            Modal::CustomPrompt(mut buf) => match key.code {
                KeyCode::Enter => {
                    let prompt = buf.trim().to_string();
                    if !prompt.is_empty() {
                        self.start_refine_custom(prompt);
                    }
                }
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    buf.pop();
                    self.modal = Modal::CustomPrompt(buf);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.modal = Modal::CustomPrompt(buf);
                }
                _ => self.modal = Modal::CustomPrompt(buf),
            },
            Modal::SymbolPicker { query, sel } => {
                let filtered = filter_symbols(&query);
                let max = filtered.len().saturating_sub(1);
                match key.code {
                    KeyCode::Enter => {
                        if let Some(&(ch, _)) = filtered.get(sel) {
                            let s = ch.to_string();
                            if self.textarea.insert_str(&s) {
                                self.dirty = true;
                                self.last_edit = Some(Instant::now());
                            }
                        }
                        // modal closes (stays Modal::None)
                    }
                    KeyCode::Esc => {}
                    KeyCode::Up => {
                        let sel = sel.saturating_sub(1);
                        self.modal = Modal::SymbolPicker { query, sel };
                    }
                    KeyCode::Down => {
                        let sel = (sel + 1).min(max);
                        self.modal = Modal::SymbolPicker { query, sel };
                    }
                    KeyCode::Backspace => {
                        let mut q = query;
                        q.pop();
                        self.modal = Modal::SymbolPicker { query: q, sel: 0 };
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        let mut q = query;
                        q.push(c);
                        self.modal = Modal::SymbolPicker { query: q, sel: 0 };
                    }
                    _ => self.modal = Modal::SymbolPicker { query, sel },
                }
            }
        }
    }
}

fn is_ctrl(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&c))
}

/// Map a raw mic RMS to a [0, 1] meter level. A square-root curve gives quiet
/// speech a visible share of the travel (RMS is tiny for normal voice) while
/// loud peaks saturate at the top.
fn normalize_level(rms: f32) -> f32 {
    (rms / LEVEL_FULL_SCALE).clamp(0.0, 1.0).sqrt()
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

// ---- dictation gesture recognizer -----------------------------------------

/// Kitty mode: a key held longer than this starts push-to-talk; shorter presses
/// count toward a double-press.
const HOLD_THRESHOLD: Duration = Duration::from_millis(300);
/// Max gap between the two presses of a double-press (both modes).
const DOUBLE_PRESS_WINDOW: Duration = Duration::from_millis(450);
/// Fallback only: presses this close together are an auto-repeat burst (a held
/// key), not two distinct taps.
const AUTOREPEAT_GAP: Duration = Duration::from_millis(140);
/// Fallback only: after a candidate second tap, wait this long with no further
/// press to be sure it wasn't the first beat of an auto-repeat burst (a hold).
const TAP_CONFIRM: Duration = Duration::from_millis(180);
/// Fallback only: once presses stop arriving for this long, the key is released.
const REPEAT_TIMEOUT: Duration = Duration::from_millis(450);

/// The three dictation gestures recognized on the configured key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GestureAction {
    StartPushToTalk,
    StopPushToTalk,
    ToggleLive,
}

/// Recognizes hold (push-to-talk) and double-press (toggle live listening) on the
/// dictation key from key events plus periodic ticks. Two code paths, chosen by
/// whether the terminal reports key releases:
///
/// - **Kitty keyboard protocol** (release events available, e.g. Ghostty): exact.
///   A second press within [`DOUBLE_PRESS_WINDOW`] toggles live; otherwise a press
///   held past [`HOLD_THRESHOLD`] starts push-to-talk and the release stops it.
///
/// - **Fallback** (no releases, e.g. macOS Terminal, tmux): only Press events
///   arrive and a held key auto-repeats, so we classify by timing. Presses within
///   [`AUTOREPEAT_GAP`] are an auto-repeat burst (a hold → push-to-talk, with the
///   release inferred from a [`REPEAT_TIMEOUT`] gap), while two *isolated* presses
///   with no following burst are a double-press (confirmed after [`TAP_CONFIRM`],
///   which lets a real hold's burst cancel a false positive first).
struct GestureState {
    binding: (KeyCode, KeyModifiers),
    release_supported: bool,
    /// A live-listening session is active (kept in sync with the worker). While
    /// live, a single press ends the session.
    live: bool,
    down: bool,
    down_since: Option<Instant>,
    /// Time of the most recent press (and, in Kitty mode, repeat).
    last_signal: Option<Instant>,
    /// Push-to-talk is currently active.
    ptt: bool,
    /// This key-down already fired a double-press toggle, so it must not also
    /// start push-to-talk.
    consumed: bool,
    /// Kitty mode: down-edge of the previous press, for double-press detection.
    prev_press: Option<Instant>,
    /// Fallback: an isolated press awaiting a possible second (double) press.
    pending_tap: Option<Instant>,
    /// Fallback: a candidate second tap, toggled to live once [`TAP_CONFIRM`]
    /// passes with no auto-repeat burst.
    double_candidate: Option<Instant>,
}

impl GestureState {
    fn new(binding: (KeyCode, KeyModifiers), release_supported: bool) -> Self {
        GestureState {
            binding,
            release_supported,
            live: false,
            down: false,
            down_since: None,
            last_signal: None,
            ptt: false,
            consumed: false,
            prev_press: None,
            pending_tap: None,
            double_candidate: None,
        }
    }

    fn set_release_supported(&mut self, supported: bool) {
        self.release_supported = supported;
    }

    /// Sync whether a live session is active. Entering live clears any lingering
    /// key episode so the next press is read cleanly as the "stop" press.
    fn set_live_active(&mut self, live: bool) {
        if live && !self.live {
            self.clear_episode();
        }
        self.live = live;
    }

    fn matches(&self, key: &KeyEvent) -> bool {
        config::key_matches(key.code, key.modifiers, &self.binding)
    }

    fn clear_episode(&mut self) {
        self.down = false;
        self.down_since = None;
        self.ptt = false;
        self.consumed = false;
        self.prev_press = None;
        self.pending_tap = None;
        self.double_candidate = None;
    }

    /// Reset to idle (e.g. when leaving the editor); emits no actions.
    fn reset(&mut self) {
        self.clear_episode();
        self.last_signal = None;
        self.live = false;
    }

    fn on_key(&mut self, kind: KeyEventKind, now: Instant) -> Option<GestureAction> {
        match kind {
            // While live, a single press ends the session (takes precedence).
            KeyEventKind::Press if self.live => self.on_press_live_stop(now),
            KeyEventKind::Press if self.release_supported => self.on_press_kitty(now),
            KeyEventKind::Press => self.on_press_fallback(now),
            // Repeat/Release events only occur under the Kitty protocol.
            KeyEventKind::Repeat => {
                self.last_signal = Some(now);
                None
            }
            KeyEventKind::Release => self.on_release_kitty(now),
        }
    }

    /// While a live session is active, the first press ends it (later auto-repeat
    /// presses of the same key-hold are ignored).
    fn on_press_live_stop(&mut self, now: Instant) -> Option<GestureAction> {
        self.last_signal = Some(now);
        if self.down {
            return None;
        }
        // Optimistically clear `live` so a fast follow-up press can't race the
        // worker's status update and toggle live straight back on.
        self.live = false;
        self.down = true;
        self.down_since = Some(now);
        self.consumed = true;
        self.ptt = false;
        self.prev_press = None;
        self.pending_tap = None;
        self.double_candidate = None;
        Some(GestureAction::ToggleLive)
    }

    fn on_tick(&mut self, now: Instant) -> Option<GestureAction> {
        if self.release_supported {
            self.on_tick_kitty(now)
        } else {
            self.on_tick_fallback(now)
        }
    }

    // --- Kitty protocol path (exact: real press/release edges) --------------

    fn on_press_kitty(&mut self, now: Instant) -> Option<GestureAction> {
        self.last_signal = Some(now);
        if self.down {
            // A held key sends Repeat events, not Press; ignore stray repeats.
            return None;
        }
        // A second press soon after the previous one is a double-press.
        if let Some(prev) = self.prev_press {
            if now.duration_since(prev) <= DOUBLE_PRESS_WINDOW {
                self.prev_press = None;
                self.down = true;
                self.down_since = Some(now);
                self.ptt = false;
                self.consumed = true;
                return Some(GestureAction::ToggleLive);
            }
        }
        self.prev_press = Some(now);
        self.down = true;
        self.down_since = Some(now);
        self.ptt = false;
        self.consumed = false;
        None
    }

    fn on_release_kitty(&mut self, now: Instant) -> Option<GestureAction> {
        self.last_signal = Some(now);
        if !self.down {
            return None;
        }
        let was_ptt = self.ptt;
        self.down = false;
        self.down_since = None;
        self.ptt = false;
        self.consumed = false;
        // `prev_press` is kept so a quick following press is a double-press.
        was_ptt.then_some(GestureAction::StopPushToTalk)
    }

    fn on_tick_kitty(&mut self, now: Instant) -> Option<GestureAction> {
        if self.down
            && !self.ptt
            && !self.consumed
            && self
                .down_since
                .is_some_and(|t| now.duration_since(t) >= HOLD_THRESHOLD)
        {
            self.ptt = true;
            self.prev_press = None;
            return Some(GestureAction::StartPushToTalk);
        }
        None
    }

    // --- Fallback path (Press events only; classify by inter-press timing) --

    fn on_press_fallback(&mut self, now: Instant) -> Option<GestureAction> {
        let gap = self.last_signal.map(|t| now.duration_since(t));
        self.last_signal = Some(now);
        self.down = true;

        // A fast burst means the key is physically held → push-to-talk.
        if gap.is_some_and(|g| g <= AUTOREPEAT_GAP) {
            self.double_candidate = None; // a burst is not a double-press
            if !self.ptt && !self.consumed {
                self.ptt = true;
                self.pending_tap = None;
                return Some(GestureAction::StartPushToTalk);
            }
            return None;
        }

        // Otherwise an isolated press: a first tap, a second tap, or the first
        // beat of a hold's auto-repeat (a following fast press will reveal which).
        if let Some(first) = self.pending_tap {
            if now.duration_since(first) <= DOUBLE_PRESS_WINDOW {
                self.pending_tap = None;
                self.double_candidate = Some(now);
                return None;
            }
        }
        self.pending_tap = Some(now);
        self.double_candidate = None;
        self.ptt = false;
        self.consumed = false;
        None
    }

    fn on_tick_fallback(&mut self, now: Instant) -> Option<GestureAction> {
        // Confirm a double-press once no auto-repeat burst has followed it.
        if let Some(t) = self.double_candidate {
            if now.duration_since(t) >= TAP_CONFIRM {
                self.double_candidate = None;
                self.pending_tap = None;
                self.consumed = true; // suppress push-to-talk if the 2nd tap is held
                return Some(GestureAction::ToggleLive);
            }
        }
        // Expire a lone first tap that never got a partner.
        if self
            .pending_tap
            .is_some_and(|t| now.duration_since(t) > DOUBLE_PRESS_WINDOW)
        {
            self.pending_tap = None;
        }
        // Infer release once presses stop arriving.
        if self
            .last_signal
            .is_some_and(|t| now.duration_since(t) >= REPEAT_TIMEOUT)
        {
            let was_ptt = self.ptt;
            self.down = false;
            self.ptt = false;
            self.consumed = false;
            return was_ptt.then_some(GestureAction::StopPushToTalk);
        }
        None
    }
}

#[cfg(test)]
mod gesture_tests {
    use super::*;

    fn binding() -> (KeyCode, KeyModifiers) {
        (KeyCode::F(5), KeyModifiers::NONE)
    }

    #[test]
    fn hold_starts_and_release_stops_push_to_talk() {
        let mut g = GestureState::new(binding(), true);
        let t0 = Instant::now();
        assert_eq!(g.on_key(KeyEventKind::Press, t0), None);
        assert_eq!(g.on_tick(t0 + Duration::from_millis(100)), None);
        assert_eq!(
            g.on_tick(t0 + Duration::from_millis(300)),
            Some(GestureAction::StartPushToTalk)
        );
        // Idempotent while held.
        assert_eq!(g.on_tick(t0 + Duration::from_millis(400)), None);
        assert_eq!(
            g.on_key(KeyEventKind::Release, t0 + Duration::from_millis(500)),
            Some(GestureAction::StopPushToTalk)
        );
    }

    #[test]
    fn quick_tap_does_not_start_push_to_talk() {
        let mut g = GestureState::new(binding(), true);
        let t0 = Instant::now();
        g.on_key(KeyEventKind::Press, t0);
        assert_eq!(
            g.on_key(KeyEventKind::Release, t0 + Duration::from_millis(80)),
            None
        );
        assert_eq!(g.on_tick(t0 + Duration::from_millis(400)), None);
    }

    #[test]
    fn double_tap_toggles_live() {
        let mut g = GestureState::new(binding(), true);
        let t0 = Instant::now();
        g.on_key(KeyEventKind::Press, t0);
        g.on_key(KeyEventKind::Release, t0 + Duration::from_millis(60));
        assert_eq!(
            g.on_key(KeyEventKind::Press, t0 + Duration::from_millis(200)),
            Some(GestureAction::ToggleLive)
        );
        assert_eq!(
            g.on_key(KeyEventKind::Release, t0 + Duration::from_millis(260)),
            None
        );
    }

    #[test]
    fn separated_taps_are_not_a_double_press() {
        let mut g = GestureState::new(binding(), true);
        let t0 = Instant::now();
        g.on_key(KeyEventKind::Press, t0);
        g.on_key(KeyEventKind::Release, t0 + Duration::from_millis(60));
        assert_eq!(
            g.on_key(KeyEventKind::Press, t0 + Duration::from_millis(900)),
            None
        );
    }

    #[test]
    fn single_press_ends_live_session() {
        // Double-press starts live; once active, a single press ends it.
        let mut g = GestureState::new(binding(), true);
        g.set_live_active(true);
        let t0 = Instant::now();
        assert_eq!(
            g.on_key(KeyEventKind::Press, t0),
            Some(GestureAction::ToggleLive)
        );
        // Held/auto-repeat of the same press must not re-toggle.
        assert_eq!(
            g.on_key(KeyEventKind::Repeat, t0 + Duration::from_millis(50)),
            None
        );
        assert_eq!(
            g.on_key(KeyEventKind::Release, t0 + Duration::from_millis(80)),
            None
        );
    }

    #[test]
    fn single_press_ends_live_session_in_fallback() {
        let mut g = GestureState::new(binding(), false);
        g.set_live_active(true);
        let t0 = Instant::now();
        assert_eq!(
            g.on_key(KeyEventKind::Press, t0),
            Some(GestureAction::ToggleLive)
        );
        // Auto-repeat presses of the same hold don't re-toggle.
        assert_eq!(
            g.on_key(KeyEventKind::Press, t0 + Duration::from_millis(40)),
            None
        );
    }

    #[test]
    fn fallback_burst_starts_and_stops_push_to_talk() {
        let mut g = GestureState::new(binding(), false);
        let t0 = Instant::now();
        // First press, then (after the OS initial delay) an auto-repeat burst.
        assert_eq!(g.on_key(KeyEventKind::Press, t0), None);
        // The first repeat looks isolated...
        assert_eq!(
            g.on_key(KeyEventKind::Press, t0 + Duration::from_millis(400)),
            None
        );
        // ...a fast second repeat reveals the key is held → push-to-talk.
        assert_eq!(
            g.on_key(KeyEventKind::Press, t0 + Duration::from_millis(440)),
            Some(GestureAction::StartPushToTalk)
        );
        // Presses stop; the release timeout ends it.
        assert_eq!(
            g.on_tick(t0 + Duration::from_millis(440 + 500)),
            Some(GestureAction::StopPushToTalk)
        );
    }

    #[test]
    fn fallback_double_press_toggles_live() {
        let mut g = GestureState::new(binding(), false);
        let t0 = Instant::now();
        assert_eq!(g.on_key(KeyEventKind::Press, t0), None);
        // A second isolated tap soon after — a double-press candidate, not a burst.
        assert_eq!(
            g.on_key(KeyEventKind::Press, t0 + Duration::from_millis(200)),
            None
        );
        // No burst follows, so after the confirm window it toggles live.
        assert_eq!(
            g.on_tick(t0 + Duration::from_millis(200 + 180)),
            Some(GestureAction::ToggleLive)
        );
    }

    #[test]
    fn fallback_hold_burst_not_mistaken_for_double() {
        let mut g = GestureState::new(binding(), false);
        let t0 = Instant::now();
        g.on_key(KeyEventKind::Press, t0);
        // The first repeat looks isolated and sets a double-press candidate...
        g.on_key(KeyEventKind::Press, t0 + Duration::from_millis(300));
        // ...but a fast repeat cancels it and starts push-to-talk instead.
        assert_eq!(
            g.on_key(KeyEventKind::Press, t0 + Duration::from_millis(340)),
            Some(GestureAction::StartPushToTalk)
        );
        // The confirm tick must NOT toggle live (the candidate was cancelled).
        assert_ne!(
            g.on_tick(t0 + Duration::from_millis(300 + 180)),
            Some(GestureAction::ToggleLive)
        );
    }

    #[test]
    fn fallback_single_press_times_out_as_a_tap() {
        let mut g = GestureState::new(binding(), false);
        let t0 = Instant::now();
        assert_eq!(g.on_key(KeyEventKind::Press, t0), None);
        assert_eq!(g.on_tick(t0 + Duration::from_millis(600)), None);
    }
}

#[cfg(test)]
mod dictation_key_tests {
    use super::*;
    use crate::storage::Store;

    fn app_with_dictation_key(key: &str) -> App {
        let dir = std::env::temp_dir().join(format!("ant-dk-{}", uuid::Uuid::new_v4()));
        let store = Store::new(dir).unwrap();
        let cfg = Config {
            dictation_key: key.to_string(),
            ..Config::default()
        };
        App::new(cfg, store)
    }

    /// The gesture recognizer must bind whatever `dictation_key` the config sets —
    /// this is the end-to-end proof that the config value (not a hard-coded
    /// default) drives the binding.
    #[test]
    fn app_binds_the_configured_dictation_key() {
        let app = app_with_dictation_key("ctrl+g");
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert!(app.gesture.matches(&ctrl('g')));
        assert!(!app.gesture.matches(&ctrl('d')));

        let app = app_with_dictation_key("F5");
        assert!(app
            .gesture
            .matches(&KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)));
        assert!(!app
            .gesture
            .matches(&KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE)));
    }
}

#[cfg(test)]
mod level_tests {
    use super::*;

    #[test]
    fn normalize_level_maps_rms_into_unit_range() {
        assert_eq!(normalize_level(0.0), 0.0);
        // Saturates at full scale and never exceeds 1 for louder-than-full input.
        assert_eq!(normalize_level(LEVEL_FULL_SCALE), 1.0);
        assert_eq!(normalize_level(LEVEL_FULL_SCALE * 4.0), 1.0);
        // The sqrt curve lifts quiet speech to a visible share of the travel.
        let quiet = normalize_level(LEVEL_FULL_SCALE * 0.25);
        assert!((quiet - 0.5).abs() < 1e-6);
    }
}
