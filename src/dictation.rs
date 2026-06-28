//! Local speech-to-text dictation.
//!
//! A single long-lived worker thread owns the whisper.cpp model and the
//! microphone, mirroring the background-thread + `mpsc` pattern used by
//! [`crate::llm`]. The UI sends [`DictationCmd`]s and polls [`DictationMsg`]s in
//! `App::on_tick`; it never blocks on transcription.
//!
//! Two capture modes:
//! - **Push-to-talk** — record everything between `StartPushToTalk` and
//!   `StopPushToTalk`, then transcribe the whole utterance.
//! - **Live** — keep listening and use a simple energy-based VAD to segment
//!   speech; each phrase is transcribed once trailing silence is detected.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Once;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use whisper_rs::{
    install_logging_hooks, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
};

use crate::audio::{self, Capture, TARGET_RATE};
use crate::config::Config;

/// How often the worker wakes to check for commands while idle / between audio
/// chunks. Bounds the latency of reacting to stop/toggle.
const POLL: Duration = Duration::from_millis(100);
/// RMS amplitude above which a chunk counts as speech (f32 samples in [-1, 1]).
const VOICE_RMS: f32 = 0.012;
/// Below this RMS the captured audio is effectively pure silence — almost always
/// means the mic delivered no signal (e.g. the terminal lacks mic permission),
/// not a quiet voice (a live room still has a noise floor well above this).
const SILENCE_RMS: f32 = 1e-4;
/// Don't bother transcribing clips shorter than this (~0.2 s of 16 kHz audio).
const MIN_TRANSCRIBE_SAMPLES: usize = TARGET_RATE as usize / 5;
/// Pre-speech audio to retain in live mode so the first word isn't clipped (~0.3 s).
const PREROLL_SAMPLES: usize = TARGET_RATE as usize * 3 / 10;
/// Hard cap on a single segment (~20 s) so a long hold / unbroken speech can't
/// grow the buffer without bound.
const MAX_SEGMENT_SAMPLES: usize = TARGET_RATE as usize * 20;

/// What the worker is currently doing (for a future UI indicator).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictationStatus {
    Idle,
    /// Loading (and possibly downloading) the speech model.
    Loading,
    /// Model loaded and ready; not currently recording.
    Ready,
    /// Push-to-talk: recording while the key is held.
    Listening,
    /// Continuous live listening.
    Live,
    /// Running whisper on a captured segment.
    Transcribing,
}

/// Message from the worker back to the UI.
pub enum DictationMsg {
    /// Recognized text to insert into the editor.
    Text(String),
    /// A status change.
    Status(DictationStatus),
    /// Current microphone input level (chunk RMS, roughly [0, ~0.3]) while
    /// capturing. Drives the animated input-level cursor in the editor.
    Level(f32),
    /// A failure (no mic, model load/download failed, transcription error).
    Error(String),
}

/// Command from the UI to the worker.
pub enum DictationCmd {
    StartPushToTalk,
    StopPushToTalk,
    ToggleLive,
    /// Download (if missing) and load the model in the background, silently —
    /// no status or error messages. Sent once at app startup so that even on a
    /// fresh install the model is fetched ahead of first use; by the time the
    /// user reaches the editor and dictates, it's already on disk and loaded.
    Prefetch,
    /// Preload the model if it's already on disk (no download), so dictation is
    /// instant and the UI can show a "ready" indicator. Sent on entering the
    /// editor. A no-op when the model file isn't present yet.
    Warmup,
    /// Stop any capture and return to idle (e.g. on leaving the editor).
    Stop,
}

/// Anything that can turn 16 kHz mono f32 PCM into text. Abstracted so the worker
/// can be exercised in tests with a mock instead of a real model + microphone.
pub trait Transcriber: Send {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String>;
}

/// whisper.cpp-backed transcriber. Loads the model once (expensive) and reuses it.
pub struct WhisperTranscriber {
    ctx: WhisperContext,
    language: String,
    threads: i32,
}

impl WhisperTranscriber {
    pub fn load(model_path: &Path, language: &str) -> Result<Self> {
        // Route whisper.cpp / GGML logging through whisper-rs's hook. With no
        // `log`/`tracing` backend wired up this effectively silences it, which
        // matters here: raw stderr prints would corrupt the alt-screen TUI.
        static INIT: Once = Once::new();
        INIT.call_once(install_logging_hooks);

        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .with_context(|| format!("loading speech model {}", model_path.display()))?;
        let threads = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 8) as i32;
        Ok(Self {
            ctx,
            language: language.to_string(),
            threads,
        })
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String> {
        let mut state = self.ctx.create_state().context("creating whisper state")?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        if self.language.eq_ignore_ascii_case("auto") {
            params.set_detect_language(true);
        } else {
            params.set_language(Some(self.language.as_str()));
        }

        // whisper.cpp wants at least ~1 s of audio; pad short clips with silence.
        let mut samples = pcm.to_vec();
        if samples.len() < TARGET_RATE as usize {
            samples.resize(TARGET_RATE as usize, 0.0);
        }
        state
            .full(params, &samples)
            .context("running whisper transcription")?;

        let mut text = String::new();
        for i in 0..state.full_n_segments() {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(s) = seg.to_str_lossy() {
                    text.push_str(&s);
                }
            }
        }
        Ok(text)
    }
}

/// Lazily constructs the transcriber on first use (so the model isn't loaded /
/// downloaded unless dictation is actually used).
type TranscriberFactory = Box<dyn FnMut() -> Result<Box<dyn Transcriber>> + Send>;

/// Spawn the dictation worker for the given config. Returns the command sender,
/// message receiver, and a join handle. Join the handle on app exit (after
/// dropping the sender) so the Metal/GPU context is fully torn down before the
/// C++ atexit handlers run — otherwise Metal asserts on process exit.
pub fn spawn(cfg: &Config) -> (Sender<DictationCmd>, Receiver<DictationMsg>, JoinHandle<()>) {
    let model_path = cfg.resolved_model_path();
    let model = cfg.dictation_model.clone();
    let language = cfg.dictation_language.clone();
    let silence_ms = cfg.dictation_silence_ms;

    let factory_path = model_path.clone();
    let factory: TranscriberFactory = Box::new(move || {
        ensure_model(&factory_path, &model)?;
        let t = WhisperTranscriber::load(&factory_path, &language)?;
        Ok(Box::new(t) as Box<dyn Transcriber>)
    });
    spawn_with_factory(factory, Some(model_path), silence_ms)
}

fn spawn_with_factory(
    factory: TranscriberFactory,
    model_path: Option<PathBuf>,
    silence_ms: u64,
) -> (Sender<DictationCmd>, Receiver<DictationMsg>, JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<DictationCmd>();
    let (msg_tx, msg_rx) = mpsc::channel::<DictationMsg>();
    let handle = thread::spawn(move || {
        // The cpal stream is created on this thread and never leaves it.
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>();
        let worker = Worker {
            factory,
            model_path,
            transcriber: None,
            msg_tx,
            audio_tx,
            audio_rx,
            capture: None,
            mode: Mode::Idle,
            buffer: Vec::new(),
            vad: Vad::new(silence_ms),
        };
        worker.run(cmd_rx);
    });
    (cmd_tx, msg_rx, handle)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    PushToTalk,
    Live,
}

struct Worker {
    factory: TranscriberFactory,
    /// Where the model file lives; lets Warmup check presence without downloading.
    model_path: Option<PathBuf>,
    transcriber: Option<Box<dyn Transcriber>>,
    msg_tx: Sender<DictationMsg>,
    audio_tx: Sender<Vec<f32>>,
    audio_rx: Receiver<Vec<f32>>,
    capture: Option<Capture>,
    mode: Mode,
    buffer: Vec<f32>,
    vad: Vad,
}

impl Worker {
    fn run(mut self, cmd_rx: Receiver<DictationCmd>) {
        loop {
            // Drain queued commands first so stop/toggle react promptly.
            loop {
                match cmd_rx.try_recv() {
                    Ok(cmd) => self.handle_cmd(cmd),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return, // App dropped.
                }
            }

            if self.capture.is_none() {
                // Idle: block on commands so we don't busy-spin.
                match cmd_rx.recv_timeout(POLL) {
                    Ok(cmd) => self.handle_cmd(cmd),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                // Capturing: consume audio (commands are re-checked at the top).
                match self.audio_rx.recv_timeout(POLL) {
                    Ok(chunk) => self.process_audio(chunk),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {}
                }
            }
        }
    }

    fn handle_cmd(&mut self, cmd: DictationCmd) {
        match cmd {
            DictationCmd::StartPushToTalk => {
                if !self.ensure_transcriber(false) {
                    return;
                }
                self.buffer.clear();
                self.vad.reset();
                self.start_capture();
                if self.capture.is_some() {
                    self.mode = Mode::PushToTalk;
                    self.send_status(DictationStatus::Listening);
                }
            }
            DictationCmd::StopPushToTalk => {
                if self.mode == Mode::PushToTalk {
                    self.stop_capture();
                    self.drain_audio();
                    let pcm = std::mem::take(&mut self.buffer);
                    self.mode = Mode::Idle;
                    self.transcribe_and_emit(pcm);
                }
            }
            DictationCmd::ToggleLive => {
                if self.mode == Mode::Live {
                    self.stop_capture();
                    self.drain_audio();
                    let pcm = std::mem::take(&mut self.buffer);
                    let was_speaking = self.vad.in_speech();
                    self.vad.reset();
                    self.mode = Mode::Idle;
                    if was_speaking {
                        self.transcribe_and_emit(pcm);
                    } else {
                        self.restore_running_status();
                    }
                } else {
                    if !self.ensure_transcriber(false) {
                        return;
                    }
                    self.buffer.clear();
                    self.vad.reset();
                    self.start_capture();
                    if self.capture.is_some() {
                        self.mode = Mode::Live;
                        self.send_status(DictationStatus::Live);
                    }
                }
            }
            DictationCmd::Prefetch => {
                // Background prefetch at startup: download the model if it isn't on
                // disk yet, then load it — all silently. Best-effort, so a failure
                // (e.g. no network) is swallowed here and only surfaces if the user
                // later actually dictates.
                self.ensure_transcriber(true);
            }
            DictationCmd::Warmup => {
                // Preload only if the model is already downloaded — never trigger a
                // multi-hundred-MB download just from opening a note.
                let ready = self.transcriber.is_some()
                    || (self.model_path.as_ref().is_some_and(|p| p.exists())
                        && self.ensure_transcriber(false));
                if ready {
                    self.send_status(DictationStatus::Ready);
                }
            }
            DictationCmd::Stop => {
                self.stop_capture();
                self.buffer.clear();
                self.vad.reset();
                self.mode = Mode::Idle;
                self.restore_running_status();
            }
        }
    }

    fn process_audio(&mut self, chunk: Vec<f32>) {
        // Report the input level for the UI meter on every captured chunk (both
        // modes), so the cursor animates to the live mic level.
        let level = rms(&chunk);
        if self.mode != Mode::Idle {
            let _ = self.msg_tx.send(DictationMsg::Level(level));
        }
        match self.mode {
            Mode::PushToTalk => {
                self.buffer.extend_from_slice(&chunk);
                if self.buffer.len() > MAX_SEGMENT_SAMPLES {
                    let overflow = self.buffer.len() - MAX_SEGMENT_SAMPLES;
                    self.buffer.drain(0..overflow);
                }
            }
            Mode::Live => {
                let voiced = level > VOICE_RMS;
                self.buffer.extend_from_slice(&chunk);
                if !voiced && !self.vad.in_speech() && self.buffer.len() > PREROLL_SAMPLES {
                    // Trim leading silence to a short pre-roll.
                    let overflow = self.buffer.len() - PREROLL_SAMPLES;
                    self.buffer.drain(0..overflow);
                }
                let phrase_ended = self.vad.observe(voiced, chunk.len());
                let too_long = self.vad.in_speech() && self.buffer.len() > MAX_SEGMENT_SAMPLES;
                if phrase_ended || too_long {
                    let pcm = std::mem::take(&mut self.buffer);
                    self.vad.reset();
                    self.transcribe_and_emit(pcm);
                }
            }
            Mode::Idle => {}
        }
    }

    /// Load the model (downloading it first if absent) on first use. Returns false
    /// on failure. When `silent`, suppresses the Loading status and the error
    /// message — used by the startup background prefetch, which must not touch the
    /// UI; the interactive paths pass `false` to keep their feedback.
    fn ensure_transcriber(&mut self, silent: bool) -> bool {
        if self.transcriber.is_some() {
            return true;
        }
        if !silent {
            self.send_status(DictationStatus::Loading);
        }
        match (self.factory)() {
            Ok(t) => {
                self.transcriber = Some(t);
                true
            }
            Err(e) => {
                if !silent {
                    let _ = self.msg_tx.send(DictationMsg::Error(format!("{e:#}")));
                    self.send_status(DictationStatus::Idle);
                }
                false
            }
        }
    }

    fn start_capture(&mut self) {
        if self.capture.is_some() {
            return;
        }
        // Discard any stale audio left from a previous session.
        while self.audio_rx.try_recv().is_ok() {}
        match audio::start(self.audio_tx.clone()) {
            Ok(cap) => self.capture = Some(cap),
            Err(e) => {
                let _ = self.msg_tx.send(DictationMsg::Error(format!("{e:#}")));
                self.mode = Mode::Idle;
                self.send_status(DictationStatus::Idle);
            }
        }
    }

    fn stop_capture(&mut self) {
        self.capture = None; // dropping the stream stops the mic
    }

    fn drain_audio(&mut self) {
        while let Ok(chunk) = self.audio_rx.try_recv() {
            self.buffer.extend_from_slice(&chunk);
        }
    }

    fn transcribe_and_emit(&mut self, pcm: Vec<f32>) {
        if pcm.len() < MIN_TRANSCRIBE_SAMPLES {
            self.restore_running_status();
            return;
        }
        // Pure-silence capture almost always means the mic delivered nothing —
        // surface the most common cause rather than silently inserting nothing.
        if rms(&pcm) < SILENCE_RMS {
            let _ = self.msg_tx.send(DictationMsg::Error(
                "no audio captured — enable microphone access for your terminal in \
                 System Settings → Privacy & Security → Microphone"
                    .to_string(),
            ));
            self.restore_running_status();
            return;
        }
        self.send_status(DictationStatus::Transcribing);
        let result = match self.transcriber.as_mut() {
            Some(t) => t.transcribe(&pcm),
            None => {
                self.restore_running_status();
                return;
            }
        };
        match result {
            Ok(text) => {
                let text = clean_transcript(&text);
                if !text.is_empty() {
                    let _ = self.msg_tx.send(DictationMsg::Text(text));
                }
            }
            Err(e) => {
                let _ = self.msg_tx.send(DictationMsg::Error(format!("{e:#}")));
            }
        }
        self.restore_running_status();
    }

    fn restore_running_status(&self) {
        let status = match self.mode {
            Mode::Live => DictationStatus::Live,
            Mode::PushToTalk => DictationStatus::Listening,
            // Back to idle: surface "ready" once the model is loaded so the editor
            // can show it's primed for the next dictation.
            Mode::Idle if self.transcriber.is_some() => DictationStatus::Ready,
            Mode::Idle => DictationStatus::Idle,
        };
        self.send_status(status);
    }

    fn send_status(&self, status: DictationStatus) {
        let _ = self.msg_tx.send(DictationMsg::Status(status));
    }
}

/// Trivial energy-based voice-activity detector used to segment live speech.
/// Tracks whether speech has started and how much trailing silence has elapsed.
struct Vad {
    voiced: bool,
    silence_run: usize,
    silence_samples: usize,
}

impl Vad {
    fn new(silence_ms: u64) -> Self {
        Vad {
            voiced: false,
            silence_run: 0,
            silence_samples: (silence_ms * TARGET_RATE as u64 / 1000) as usize,
        }
    }

    fn reset(&mut self) {
        self.voiced = false;
        self.silence_run = 0;
    }

    fn in_speech(&self) -> bool {
        self.voiced
    }

    /// Feed one chunk; returns true if a phrase just ended (enough trailing
    /// silence followed detected speech).
    fn observe(&mut self, voiced_chunk: bool, len: usize) -> bool {
        if voiced_chunk {
            self.voiced = true;
            self.silence_run = 0;
            false
        } else if self.voiced {
            self.silence_run += len;
            self.silence_run >= self.silence_samples
        } else {
            false
        }
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Non-speech sounds whisper sometimes annotates inside parentheses.
const NON_SPEECH_SOUNDS: &[&str] = &[
    "silence",
    "blank",
    "inaudible",
    "music",
    "noise",
    "pause",
    "applause",
    "laughter",
    "background",
    "beep",
    "click",
    "buzzing",
    "static",
    "sound",
    "wind",
    "breathing",
    "typing",
];

/// Strip whisper's non-speech annotations so they never land in the note —
/// `[BLANK_AUDIO]`, `[silence]`, `(music)`, etc. Square-bracket groups are always
/// annotations; parenthesized groups are dropped only when they name a non-speech
/// sound (so a genuinely dictated aside in parentheses is kept). Returns the
/// cleaned, whitespace-collapsed text (empty when nothing speech-like remains).
fn clean_transcript(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                // Drop the whole bracketed annotation.
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
                i += 1; // consume ']' (or run off the end)
            }
            '(' => {
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && chars[j] != ')' {
                    j += 1;
                }
                let inner: String = chars[start..j].iter().collect();
                let lower = inner.to_lowercase();
                if NON_SPEECH_SOUNDS.iter().any(|k| lower.contains(k)) {
                    i = j + 1; // drop the (sound) group
                } else {
                    out.push('(');
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Download the GGML model to `path` if it isn't already present.
fn ensure_model(path: &Path, model: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating model directory {}", parent.display()))?;
    }
    let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{model}.bin");
    let resp = ureq::get(&url)
        .call()
        .with_context(|| format!("downloading speech model from {url}"))?;

    // Write to a temp file then rename, so an interrupted download can't leave a
    // truncated model that looks valid on the next run.
    let tmp = path.with_extension("bin.partial");
    let mut file = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    std::io::copy(&mut resp.into_reader(), &mut file).context("writing speech model to disk")?;
    file.sync_all().ok();
    fs::rename(&tmp, path).with_context(|| format!("finalizing model at {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    struct MockTranscriber {
        text: String,
    }
    impl Transcriber for MockTranscriber {
        fn transcribe(&mut self, _pcm: &[f32]) -> Result<String> {
            Ok(self.text.clone())
        }
    }

    fn test_worker(silence_ms: u64, text: &str) -> (Worker, Receiver<DictationMsg>) {
        let (msg_tx, msg_rx) = mpsc::channel();
        let (audio_tx, audio_rx) = mpsc::channel();
        let worker = Worker {
            factory: Box::new(|| Err(anyhow!("factory unused in tests"))),
            model_path: None,
            transcriber: Some(Box::new(MockTranscriber {
                text: text.to_string(),
            })),
            msg_tx,
            audio_tx,
            audio_rx,
            capture: None,
            mode: Mode::Idle,
            buffer: Vec::new(),
            vad: Vad::new(silence_ms),
        };
        (worker, msg_rx)
    }

    fn collect_text(rx: &Receiver<DictationMsg>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let DictationMsg::Text(t) = msg {
                out.push(t);
            }
        }
        out
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[0.0; 100]), 0.0);
        assert!(rms(&[0.5; 100]) > VOICE_RMS);
    }

    #[test]
    fn clean_transcript_strips_non_speech_annotations() {
        assert_eq!(clean_transcript("[BLANK_AUDIO]"), "");
        assert_eq!(clean_transcript("[ Silence ]"), "");
        assert_eq!(clean_transcript("(music)"), "");
        assert_eq!(clean_transcript("Hello [silence] world"), "Hello world");
        assert_eq!(clean_transcript("  buy   milk  "), "buy milk");
        // A genuine parenthetical aside is preserved.
        assert_eq!(clean_transcript("call mom (urgent)"), "call mom (urgent)");
    }

    #[test]
    fn vad_needs_speech_before_ending_a_phrase() {
        let mut vad = Vad::new(700); // 700 ms => 11_200 samples at 16 kHz
                                     // Silence before any speech never ends a phrase.
        assert!(!vad.observe(false, 16_000));
        assert!(!vad.in_speech());
        // Speech, then enough trailing silence, ends the phrase.
        assert!(!vad.observe(true, 1_600));
        assert!(vad.in_speech());
        assert!(!vad.observe(false, 5_000));
        assert!(vad.observe(false, 7_000)); // total silence 12_000 >= 11_200
    }

    #[test]
    fn push_to_talk_emits_one_transcript() {
        let (mut w, rx) = test_worker(700, "hello world");
        w.mode = Mode::PushToTalk;
        // A held utterance of voiced audio (> MIN_TRANSCRIBE_SAMPLES).
        w.process_audio(vec![0.5; 8_000]);
        // Releasing transcribes the whole buffer.
        let pcm = std::mem::take(&mut w.buffer);
        w.mode = Mode::Idle;
        w.transcribe_and_emit(pcm);
        assert_eq!(collect_text(&rx), vec!["hello world".to_string()]);
    }

    #[test]
    fn live_mode_emits_a_transcript_per_phrase() {
        let (mut w, rx) = test_worker(700, "one phrase");
        w.mode = Mode::Live;
        // Speech...
        w.process_audio(vec![0.5; 4_000]);
        // ...followed by enough silence to end the phrase (>= 11_200 samples).
        w.process_audio(vec![0.0; 12_000]);
        assert_eq!(collect_text(&rx), vec!["one phrase".to_string()]);
        // Buffer reset, ready for the next phrase.
        assert!(w.buffer.is_empty());
    }

    #[test]
    fn live_mode_ignores_pure_silence() {
        let (mut w, rx) = test_worker(700, "should not appear");
        w.mode = Mode::Live;
        w.process_audio(vec![0.0; 30_000]);
        assert!(collect_text(&rx).is_empty());
    }
}
