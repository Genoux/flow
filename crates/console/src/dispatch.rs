//! What happens on each `Message`.
//!
//! Split from main.rs only for size: this is one match, and it is the whole
//! reason the console has state at all. Kept together because the arms share
//! ordering assumptions - a save that must land before a reload, a setup step
//! that must not run twice - which are far easier to check in one list.

use crate::*;
use iced::Task;

impl Console {
    /// Start the microphone dialog's way out, unless it is already on it - a
    /// second Escape, or an Escape landing on a click-off already in flight,
    /// must not restart the fade from full.
    fn close_picker(&mut self) {
        if matches!(self.picking_input, Some(Picker::Opening(_))) {
            self.picking_input = Some(Picker::Closing(std::time::Instant::now()));
        }
    }

    pub(crate) fn refresh_peripherals(&mut self) -> Task<Message> {
        if self.peripherals_pending {
            return Task::none();
        }
        self.peripherals_pending = true;
        Task::perform(async { Peripherals::read() }, Message::PeripheralsLoaded)
    }

    pub(crate) fn refresh_history(&mut self) -> Task<Message> {
        if self.history_pending {
            self.history_dirty = true;
            return Task::none();
        }
        self.history_pending = true;
        Task::perform(
            async { (history::recent(), history::daily(CALENDAR_DAYS)) },
            |(entries, days)| Message::HistoryLoaded(entries, days),
        )
    }

    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::BannersAllocated(results) => {
                self.banners_ready = true;
                for result in results {
                    match result {
                        Ok(allocation) => self.banner_allocations.push(allocation),
                        Err(error) => eprintln!("could not preload banner: {error}"),
                    }
                }
                self.now = std::time::Instant::now();
                self.page_motion.reveal();
            }
            Message::Select(section) => {
                if self.section != section && self.banners_ready {
                    self.now = std::time::Instant::now();
                    self.page_motion.reveal();
                }
                self.section = section;
                let now = std::time::Instant::now();
                for hover in &mut self.cleanup_hover {
                    hover.set(0.0, now);
                }
                for hover in self.entry_motion.values_mut() {
                    hover.set(0.0, now);
                }
                if section == Section::Settings {
                    return self.refresh_peripherals();
                }
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
                self.page_motion.advance(elapsed);
                if let Some(state) = self.download.as_mut() {
                    state.advance(elapsed);
                }
                // Off the tree already - `view` stopped drawing it when the
                // fade ran out. This is only the state catching up, and it has
                // to happen here because nothing else will ask again once
                // `moving` goes quiet.
                if self.picking_input.is_some_and(|picker| picker.spent(now)) {
                    self.picking_input = None;
                }
                let mut finished = Task::none();
                if let Some(fading) = self.fading.as_mut() {
                    *fading += elapsed;
                    if *fading >= setup::FADE {
                        finished = self.leave_setup();
                    }
                }
                return Task::batch([finished, self.launch_install(), self.setup_usable()]);
            }
            Message::Hover(section) => {
                let now = std::time::Instant::now();
                for (index, item) in Section::ALL.into_iter().enumerate() {
                    self.nav_motion[index].set(if section == Some(item) { 1.0 } else { 0.0 }, now);
                }
            }
            Message::HoverCleanup(level) => {
                let now = std::time::Instant::now();
                for (index, item) in settings::Cleanup::ALL.into_iter().enumerate() {
                    self.cleanup_hover[index].set(
                        if level == Some(item) && self.settings.cleanup != item {
                            1.0
                        } else {
                            0.0
                        },
                        now,
                    );
                }
            }
            Message::HoverEntry(index) => {
                let now = std::time::Instant::now();
                for (item, transition) in &mut self.entry_motion {
                    transition.set(if Some(*item) == index { 1.0 } else { 0.0 }, now);
                }
                if let Some(index) = index {
                    self.entry_motion
                        .entry(index)
                        .or_insert_with(|| motion::Transition::new(0.0))
                        .set(1.0, now);
                }
            }
            Message::SettingsSaved(result) => {
                self.save_pending = false;
                self.save_error = result.err();
                if std::mem::take(&mut self.save_dirty) {
                    return self.persist();
                }
                if let Some(window) = self.closing_window.take() {
                    if self.save_error.is_none() {
                        return iced::window::close(window);
                    }
                }
            }
            Message::CloseRequested(window) => {
                if self.save_pending {
                    self.closing_window = Some(window);
                } else {
                    return iced::window::close(window);
                }
            }
            Message::PushToTalk(on) => {
                self.animate_toggle("push_to_talk", self.settings.push_to_talk, on);
                self.settings.push_to_talk = on;
                return self.persist();
            }
            Message::SetCleanup(level) => {
                self.settings.cleanup = level;
                let now = std::time::Instant::now();
                for (index, item) in settings::Cleanup::ALL.into_iter().enumerate() {
                    self.cleanup_selection[index].set(if level == item { 1.0 } else { 0.0 }, now);
                    self.cleanup_hover[index].set(0.0, now);
                }
                return self.persist();
            }
            Message::Denoise(on) => {
                self.animate_toggle("denoise", self.settings.denoise, on);
                self.settings.denoise = on;
                return self.persist();
            }
            Message::Sound(on) => {
                self.animate_toggle("sound", self.settings.sound, on);
                self.settings.sound = on;
                return self.persist();
            }
            Message::ShowTray(on) => {
                self.animate_toggle("show_tray", self.settings.show_tray, on);
                self.settings.show_tray = on;
                let saved = self.persist();
                return if on {
                    Task::batch([
                        saved,
                        Task::perform(async { system::start_tray() }, Message::TrayStarted),
                    ])
                } else {
                    saved
                };
            }
            Message::TrayStarted(result) => {
                if let Err(err) = result {
                    self.save_error = Some(err);
                }
            }
            Message::Duck(value) => {
                self.settings.duck = value;
                return self.persist();
            }
            Message::InputDevice(name) => {
                self.settings.input_device = name;
                // Fades out rather than snapping: the row behind updates on the
                // same frame, so the dialog leaving is what shows the choice
                // landing instead of hiding it.
                self.close_picker();
                return self.persist();
            }
            Message::PickInput => {
                self.picking_input = Some(Picker::Opening(std::time::Instant::now()));
                return self.refresh_peripherals();
            }
            Message::ClosePicker => self.close_picker(),
            Message::Autostart(on) => {
                if self.autostart_pending {
                    return Task::none();
                }
                self.autostart_pending = true;
                return Task::perform(
                    async move {
                        system::set_autostart(on)?;
                        Ok(system::autostart_enabled())
                    },
                    Message::AutostartFinished,
                );
            }
            Message::AutostartFinished(result) => {
                self.autostart_pending = false;
                match result {
                    Ok(enabled) => {
                        self.animate_toggle(
                            "autostart",
                            self.autostart.unwrap_or(false),
                            enabled.unwrap_or(false),
                        );
                        self.autostart = enabled;
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
                    return self.refresh_history();
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
            Message::FilterTerms(query) => self.term_query = query,
            Message::AddTerm => match vocabulary::validate(&self.typing, &self.terms) {
                Ok(term) => {
                    let mut terms = self.terms.clone();
                    terms.push(term);
                    match vocabulary::save(&terms) {
                        Ok(()) => {
                            self.terms = terms;
                            self.typing.clear();
                            self.term_query.clear();
                            self.term_error = None;
                            return iced::widget::operation::focus("vocabulary-entry");
                        }
                        Err(err) => self.term_error = Some(err.to_string()),
                    }
                }
                Err(why) => self.term_error = Some(why),
            },
            Message::RemoveTerm(index) => {
                if index < self.terms.len() {
                    let mut terms = self.terms.clone();
                    terms.remove(index);
                    match vocabulary::save(&terms) {
                        Ok(()) => {
                            self.terms = terms;
                            self.term_error = None;
                        }
                        Err(err) => self.term_error = Some(err.to_string()),
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
                return self.persist();
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
                    return self.persist();
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
            Message::InstallChecked(damage) => self.damage = damage,
            Message::HistoryLoaded(entries, days) => {
                self.history_pending = false;
                self.entries = entries;
                self.days = days;
                self.copied = None;
                self.entry_motion.clear();
                if std::mem::take(&mut self.history_dirty) {
                    return self.refresh_history();
                }
            }
            Message::PeripheralsLoaded(peripherals) => {
                self.peripherals_pending = false;
                self.input = peripherals.input;
                self.sources = peripherals.sources;
                self.can_capture = peripherals.can_capture;
            }
            Message::OpenConfig => {
                return Task::perform(
                    async { system::open(&settings::config_path()) },
                    Message::PathOpened,
                );
            }
            Message::OpenPath(path) => {
                return Task::perform(async move { system::reveal(&path) }, Message::PathOpened);
            }
            Message::PathOpened(result) => {
                if let Err(err) = result {
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
            Message::BeginSetup => {
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
                        return self.leave_setup();
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
                    return self.leave_setup();
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
                    // waiting to be dismissed - after the beat its closing line
                    // needs to be read. Negative, so the screen stands still
                    // for `HOLD` and then runs the usual outro.
                    self.fading = Some(-setup::HOLD);
                }
            }
        }
        Task::none()
    }
}
