//! What happens on each `Message`.
//!
//! Split from main.rs only for size: this is one match, and it is the whole
//! reason the console has state at all. Kept together because the arms share
//! ordering assumptions - a save that must land before a reload, a setup step
//! that must not run twice - which are far easier to check in one list.

use crate::*;
use iced::Task;

impl Console {
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        // The socket is the one thing the demo cannot talk over: it is either
        // absent or belongs to a real daemon, and either way its first event
        // would replace the state being looked at.
        if self.demo && matches!(message, Message::Daemon(_)) {
            return Task::none();
        }

        match message {
            Message::Select(section) => {
                self.section = section;
            }
            Message::Tick(now) => {
                // The gap since the last frame, which is what the bar's easing
                // needs: a fraction-per-frame chase would settle at a different
                // speed on a 60Hz screen than on a 144Hz one.
                //
                // Capped, because `self.now` only moves on a frame and frames
                // are only asked for while something is animating. A compositor
                // hitch, or the first tick after the window maps, can be much
                // longer than a frame; a chase given that as one step arrives
                // inside it, which is the snap the speed limit exists to prevent.
                let elapsed = now
                    .saturating_duration_since(self.now)
                    .as_secs_f32()
                    .min(FRAME_CAP);
                self.now = now;
                if let Some(state) = self.download.as_mut() {
                    state.advance(elapsed);
                }
                if let Some(fading) = self.fading.as_mut() {
                    *fading += elapsed;
                    if *fading >= setup::FADE {
                        self.leave_setup();
                    }
                }
                return Task::batch([self.launch_install(), self.setup_usable()]);
            }
            Message::Hover(section) => {
                if self.hovered != section {
                    self.hovered = section;
                    self.hover_at = std::time::Instant::now();
                }
            }
            Message::HoverEntry(index) => {
                if self.hovered_entry != index {
                    self.hovered_entry = index;
                    self.entry_hover_at = std::time::Instant::now();
                }
            }
            Message::PushToTalk(on) => {
                self.settings.push_to_talk = on;
                self.toggled_at
                    .insert("push_to_talk", std::time::Instant::now());
                self.persist();
            }
            Message::SetCleanup(level) => {
                self.settings.cleanup = level;
                self.toggled_at.insert("cleanup", std::time::Instant::now());
                self.persist();
            }
            Message::Terminal(on) => {
                self.settings.terminal = on;
                self.toggled_at
                    .insert("terminal", std::time::Instant::now());
                self.persist();
            }
            Message::Denoise(on) => {
                self.settings.denoise = on;
                self.toggled_at.insert("denoise", std::time::Instant::now());
                self.persist();
            }
            Message::Sound(on) => {
                self.settings.sound = on;
                self.toggled_at.insert("sound", std::time::Instant::now());
                self.persist();
            }
            Message::Duck(value) => {
                self.settings.duck = value;
                self.persist();
            }
            Message::Autostart(on) => {
                self.toggled_at
                    .insert("autostart", std::time::Instant::now());
                match system::set_autostart(on) {
                    // Re-read rather than assume: systemd is the authority on
                    // whether that worked, not our optimism.
                    Ok(()) => {
                        self.autostart = system::autostart_enabled();
                        self.save_error = None;
                    }
                    Err(err) => self.save_error = Some(err),
                }
            }
            Message::Daemon(daemon::Event::Line(line)) => {
                let before = self.daemon.words;
                self.daemon.apply(&line);
                // Re-read the file rather than trust the socket's copy: the
                // file is what this window shows, and it is the thing that
                // outlives the daemon.
                if self.daemon.words != before {
                    self.entries = history::recent();
                    self.days = history::daily(CALENDAR_DAYS);
                }
            }
            Message::Copy(index) => {
                if let Some(text) = self.entries.get(index).map(|entry| entry.text.clone()) {
                    self.copied = Some((index, std::time::Instant::now()));
                    return iced::clipboard::write(text);
                }
            }
            Message::Daemon(daemon::Event::Disconnected) => {
                if believe_disconnect(self.daemon.activity) {
                    self.daemon = daemon::State::default();
                }
            }
            Message::TypingTerm(text) => {
                self.typing = text;
                self.term_error = None;
            }
            Message::AddTerm => match vocabulary::validate(&self.typing, &self.terms) {
                Ok(term) => {
                    self.terms.push(term);
                    self.typing.clear();
                    self.term_error = None;
                    if let Err(err) = vocabulary::save(&self.terms) {
                        self.term_error = Some(err.to_string());
                    }
                }
                Err(why) => self.term_error = Some(why),
            },
            Message::RemoveTerm(index) => {
                if index < self.terms.len() {
                    self.terms.remove(index);
                    if let Err(err) = vocabulary::save(&self.terms) {
                        self.term_error = Some(err.to_string());
                    }
                }
            }
            Message::CaptureChord => {
                self.capturing = true;
                self.chord_error = None;
                let cancelled = std::sync::Arc::clone(&self.cancel_capture);
                cancelled.store(false, std::sync::atomic::Ordering::Relaxed);
                // Off the UI thread: this blocks on the keyboard until a chord
                // arrives or the user gives up.
                return Task::perform(
                    async move { tokio_free_capture(cancelled) },
                    Message::Captured,
                );
            }
            Message::ResetChord => {
                self.settings.hotkey = settings::DEFAULT_HOTKEY.to_string();
                self.chord_error = None;
                self.persist();
            }
            Message::CancelCapture => {
                self.capturing = false;
                self.cancel_capture
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Message::Captured(captured) => {
                self.capturing = false;
                // A None is a cancel, or no readable keyboard. The control is
                // hidden in the second case, so it is nearly always the first.
                if let Some(chord) = captured {
                    self.settings.hotkey = chord;
                    self.persist();
                }
            }
            Message::Service(verb) => {
                if self.service_pending.is_some() {
                    return Task::none();
                }
                self.service_pending = Some(verb);
                self.service_error = None;
                if verb == "start" {
                    self.daemon.activity = daemon::Activity::Starting;
                }
                return Task::perform(async move { system::service(verb) }, move |result| {
                    Message::ServiceFinished(verb, result)
                });
            }
            Message::ServiceFinished(verb, result) => {
                self.service_pending = None;
                match result {
                    Ok(()) => {
                        self.service_error = None;
                        if verb == "stop" {
                            self.daemon = daemon::State::default();
                        }
                    }
                    Err(err) => {
                        self.service_error = Some(err);
                        if verb == "start" {
                            self.daemon = daemon::State::default();
                        }
                    }
                }
            }
            Message::OpenConfig => {
                if let Err(err) = system::open(&settings::config_path()) {
                    self.save_error = Some(err);
                }
            }
            Message::OpenPath(path) => {
                if let Err(err) = system::reveal(&path) {
                    self.save_error = Some(err);
                }
            }
            Message::CheckUpdate => {
                if self.update != update::Status::Checking {
                    self.update = update::Status::Checking;
                    return Task::perform(async { update::latest() }, Message::UpdateChecked);
                }
            }
            Message::UpdateChecked(status) => self.update = status,
            Message::InstallUpdate => {
                if let (false, update::Status::Available(tag)) = (self.updating, &self.update) {
                    let tag = tag.clone();
                    self.updating = true;
                    self.save_error = None;
                    return Task::perform(
                        async move { update::install(&tag).map(|()| tag) },
                        Message::UpdateInstalled,
                    );
                }
            }
            Message::UpdateInstalled(result) => {
                self.updating = false;
                match result {
                    Ok(tag) => self.update = update::Status::Installed(tag),
                    Err(err) => self.save_error = Some(err),
                }
            }
            Message::RerunSetup => {
                // Stop first: the daemon holds both models open, and the point
                // of this is to watch setup fetch them rather than to leave a
                // process running on files that no longer exist.
                let _ = system::service("stop");
                match system::remove_models() {
                    Ok(()) => {
                        self.models = system::models();
                        self.daemon = daemon::State::default();
                        return Task::done(Message::BeginSetup);
                    }
                    Err(err) => self.service_error = Some(err),
                }
            }
            Message::BeginSetup => {
                // Demo mode never spawns the installer: the point of it is to
                // lay this screen out on a machine that has no `flow` binary
                // at all, and a failure line is not the state worth looking at.
                if self.demo {
                    self.download = Some(demo::setup_state());
                    self.showing_setup = true;
                    return Task::none();
                }

                // Already on setup (a failed fetch, Try again): the veil is
                // up, so skip the intro and start fetching.
                let skip = self.showing_setup && self.download.is_some();
                let mut state = setup::State::new(setup::Handle::default());
                if skip {
                    state.skip_intro();
                }
                self.download = Some(state);
                self.showing_setup = true;
                return Task::none();
            }
            Message::StopDownload => {
                if let Some(state) = self.download.as_mut() {
                    state.stopped = true;
                    state.handle.stop();
                    // Stopped before the installer was even spawned, so no
                    // `Failed` line is coming to carry the handover. Nothing was
                    // fetched this run either - and a part file here belongs to
                    // an earlier one, which is exactly what a resume is for.
                    if !state.spawned {
                        self.leave_setup();
                    }
                }
            }
            Message::SetupEvent(event) => {
                if let Some(state) = self.download.as_mut() {
                    state.apply(event);
                }

                let over = !self
                    .download
                    .as_ref()
                    .is_some_and(setup::State::downloading);
                let stopped = over && self.download.as_ref().is_some_and(|state| state.stopped);

                // What was downloaded stays downloaded. Stopping used to delete
                // the part file, on the reasoning that bytes of a model someone
                // had decided against would sit there with nothing on screen
                // ever mentioning them - but neither half of that is true any
                // more. Flow needs both models, so there is no deciding against
                // one; and Overview carries a banner saying setup is unfinished
                // with the button that finishes it. The bytes are accounted for.
                //
                // What is left is a 2.4 GB download where Stop threw away
                // everything already fetched. `curl -C -` resumes, so keeping
                // the file makes Stop mean "not now" instead of "start again".
                //
                // It still hands the window over rather than holding them on a
                // ring that failed: the console opens, incomplete, saying what
                // is missing and offering to finish. Treating a stop as a
                // failure would put a Try again in front of the one person who
                // has already said no.
                if stopped {
                    self.leave_setup();
                    return Task::none();
                }

                // Setup keeps its state until it has faded out; anything else
                // is done with the moment it stops, and what is on disk has
                // just changed.
                if over && !self.showing_setup {
                    self.download = None;
                    self.models = system::models();
                }

                return self.setup_usable();
            }
            Message::SetupStarted(result) => {
                let started = result.is_ok();
                if let Some(state) = self.download.as_mut() {
                    state.starting_daemon = false;
                    match result {
                        Ok(()) => {
                            state.daemon_started = true;
                            state.start_error = None;
                        }
                        Err(err) => state.start_error = Some(err),
                    }
                }

                if started {
                    self.daemon.activity = daemon::Activity::Starting;
                    // What the veil is about to reveal has to be true before it
                    // starts moving, not after. `models` was refreshed only in
                    // `leave_setup`, which runs when the fade ends - so Overview
                    // spent the whole dissolve showing the "setup isn't
                    // finished" banner for the setup that had just finished,
                    // then dropped it as the veil landed.
                    self.models = system::models();
                    // Setup's whole job is done, so it dissolves rather than
                    // waiting to be dismissed. There is nothing on it to read.
                    self.fading = Some(0.0);
                }
            }
        }
        Task::none()
    }
}
