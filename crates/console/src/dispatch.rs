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

    fn cancel_chord_capture(&mut self) {
        self.capturing = false;
        self.cancel_capture
            .store(true, std::sync::atomic::Ordering::Relaxed);
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
                self.cancel_chord_capture();
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
                if std::mem::take(&mut self.restart_pending) && self.save_error.is_none() {
                    return self.update(Message::RestartApp);
                }
                if let Some(window) = self.closing_window.take() {
                    if self.save_error.is_none() {
                        return iced::window::close(window);
                    }
                }
            }
            Message::CloseRequested(window) => {
                self.cancel_chord_capture();
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
            Message::SetChannel(experimental) => {
                if self.updating {
                    return Task::none();
                }
                let wanted = if experimental {
                    system::Channel::Experimental
                } else {
                    system::Channel::Stable
                };
                self.updating = true;
                self.save_error = None;
                return Task::perform(
                    async move { update::join_channel(wanted) },
                    Message::ChannelInstalled,
                );
            }
            Message::ChannelInstalled(result) => {
                self.updating = false;
                match result {
                    Ok(()) => {
                        self.channel = system::channel();
                        self.update = update::Status::Installed(self.channel.suffix().into());
                    }
                    Err(error) => self.save_error = Some(error),
                }
            }
            // The banner's one action. A missing key just needs Settings; a
            // missing or damaged speech model needs the actual download, so
            // that is the only case that opens the setup screen.
            Message::BeginSetup => {
                if self.install_problem().is_none() {
                    self.section = Section::Settings;
                    return Task::none();
                }
                // Already on setup (a failed fetch, Try again): the veil is
                // up, so skip the intro and start fetching.
                let skip = self.showing_setup && self.download.is_some();
                let mut state = setup::State::new();
                if skip {
                    state.skip_intro();
                }
                self.download = Some(state);
                self.showing_setup = true;
                return Task::none();
            }
            Message::SetupEvent(event) => {
                if let Some(state) = self.download.as_mut() {
                    state.apply(event);
                }

                let over = !self
                    .download
                    .as_ref()
                    .is_some_and(setup::State::downloading);

                // Setup keeps its state until it has faded out; anything else
                // is done with the moment it stops, and what is on disk has
                // just changed.
                if over && !self.showing_setup {
                    self.download = None;
                    self.damage = system::startup_damage();
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
                    // starts moving, not after. `damage` was refreshed only in
                    // `leave_setup`, which runs when the fade ends - so Overview
                    // spent the whole dissolve showing the "setup isn't
                    // finished" banner for the setup that had just finished,
                    // then dropped it as the veil landed.
                    self.damage = system::startup_damage();
                    // Setup's whole job is done, so it dissolves rather than
                    // waiting to be dismissed - after the beat its closing line
                    // needs to be read. Negative, so the screen stands still
                    // for `HOLD` and then runs the usual outro.
                    self.fading = Some(-setup::HOLD);
                }
            }
            Message::InstallChecked(damage) => self.damage = damage,
            Message::TypingKey(value) => {
                self.typing_key = value;
                self.key_error = None;
            }
            Message::SaveKey => match openrouter::validate(&self.typing_key) {
                Ok(key) => {
                    self.settings.openrouter_key = Some(key);
                    self.typing_key.clear();
                    self.key_error = None;
                    self.key_test = None;
                    // Saving a credential and finding out whether it works are
                    // one intention, so the save starts the check itself.
                    return Task::batch([self.persist(), Task::done(Message::TestKey)]);
                }
                Err(err) => self.key_error = Some(err),
            },
            Message::ClearKey => {
                self.settings.openrouter_key = None;
                self.typing_key.clear();
                self.key_error = None;
                self.key_test = None;
                // A level that needs a key it no longer has would keep being
                // selected while silently pasting the raw transcript, so the
                // level comes down with the key rather than being left to fail.
                if self.settings.cleanup.needs_key() {
                    self.settings.cleanup = crate::settings::Cleanup::None;
                }
                return self.persist();
            }
            Message::TestKey => {
                if self.testing_key || self.settings.openrouter_key.is_none() {
                    return Task::none();
                }
                self.testing_key = true;
                return Task::perform(async { system::probe_router() }, Message::KeyTested);
            }
            Message::KeyTested(outcome) => {
                self.testing_key = false;
                self.key_test = outcome;
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
                self.cancel_chord_capture();
                self.capture_id = self.capture_id.wrapping_add(1);
                let id = self.capture_id;
                self.capturing = true;
                self.chord_error = None;
                self.cancel_capture =
                    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let cancelled = std::sync::Arc::clone(&self.cancel_capture);
                return Task::perform(
                    async move { tokio_free_capture(cancelled) },
                    move |result| Message::Captured(id, result),
                );
            }
            Message::ResetChord => {
                self.cancel_chord_capture();
                self.settings.hotkey = settings::DEFAULT_HOTKEY.to_string();
                self.chord_error = None;
                return self.persist();
            }
            Message::CancelCapture => self.cancel_chord_capture(),
            Message::Captured(id, captured) => {
                if !self.capturing || id != self.capture_id {
                    return Task::none();
                }
                self.cancel_chord_capture();
                match captured {
                    Ok(Some(chord)) => {
                        self.settings.hotkey = chord;
                        return self.persist();
                    }
                    Ok(None) => {}
                    Err(error) => self.chord_error = Some(error),
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
            Message::UpdateChecked(status) => {
                if !self.updating && !matches!(self.update, update::Status::Installed(_)) {
                    self.update = status;
                }
            }
            Message::RestartApp => {
                if self.updating || self.save_pending {
                    return Task::none();
                }
                self.updating = true;
                return Task::perform(async { system::restart_app() }, Message::AppRestarted);
            }
            Message::AppRestarted(result) => {
                self.updating = false;
                match result {
                    Ok(()) => return iced::exit(),
                    Err(error) => self.save_error = Some(error),
                }
            }
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
                    Ok(tag) => {
                        self.update = update::Status::Installed(tag);
                        if self.save_pending {
                            self.restart_pending = true;
                        } else {
                            return self.update(Message::RestartApp);
                        }
                    }
                    Err(err) => self.save_error = Some(err),
                }
            }
        }
        Task::none()
    }
}
