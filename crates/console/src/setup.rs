//! First run, and repair: one model, one ring, then the console.
//!
//! A fresh install has no speech model on disk, and Flow cannot dictate
//! without it - so the window opens on this instead of on seven sections
//! reporting nothing. It fetches Nemotron, starts the daemon, and fades into
//! the Overview. There is nothing to read and nothing to press.
//!
//! The same screen runs when a file the daemon pins goes missing or comes
//! back the wrong length: `flow check --porcelain` finds that at launch, and
//! Overview's banner offers the same download under a different name, Repair.
//! Refining is not this screen's business - it lives behind an OpenRouter key
//! on the Settings screen, and a missing key never blocks dictation the way a
//! missing recogniser does.
//!
//! The download itself is `flow install --porcelain`, not a copy of it. The
//! pinned revision and sha256s live in the daemon's `install.rs` and are the
//! reason an install can be trusted; a second downloader in the window would
//! be a second place for them to be wrong. The size comes from the same
//! manifest, through `--plan`, for the same reason.

use crate::theme::{ease_out, emerge, ACCENT, BG, ERR, FAINT, LINE, MUTED};
use crate::Message;
use iced::widget::canvas::{self, Canvas, Path, Stroke};
use iced::widget::{column, container, stack, text, Space};
use iced::{mouse, Element, Fill, Length, Point, Radians, Rectangle, Renderer, Theme};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// Whether this machine still needs setting up.
///
/// The coarse check for a launch that should go straight to this screen with
/// nothing clicked: no speech model directory at all. A directory that exists
/// but holds a damaged file is not this - the console can still open, and the
/// fix is offered from Overview's banner instead, under Repair.
pub fn needed() -> bool {
    !flow_paths::speech_model_dir().is_dir()
}

/// One line of `flow install --porcelain`, or the end of it.
#[derive(Debug, Clone)]
pub enum Event {
    /// Total bytes this install will fetch.
    Total(u64),
    /// Hashing, either checking what is on disk or verifying what arrived.
    Verifying(String),
    /// Now downloading this file.
    Fetching(String),
    /// Bytes done.
    Progress(u64),
    /// A file landed. Carries no name because the ring is driven by bytes.
    Installed,
    Finished,
    Failed(String),
}

/// Run the installer and stream what it says.
///
/// Same shape as `daemon::stream`: a blocking read on its own thread feeding an
/// unbounded channel. The protocol is one short line per change and the reader
/// has nothing else to do, so a thread is both less code and easier to follow
/// than making this cooperate with the GUI executor.
///
/// There is no way to cancel this from the caller, and no `Drop` reaches back
/// to kill it either - the model is not optional, so there is nothing a user
/// could decide against once this has started. If the window closes
/// mid-download, `flow install --porcelain` keeps running and finishes
/// unattended: the fetch is idempotent and `curl -C -` resumes, so an orphaned
/// install completing the thing Flow actually needs is the outcome, not a
/// leak to guard against.
pub fn install() -> impl iced::futures::Stream<Item = Event> {
    let (tx, rx) = iced::futures::channel::mpsc::unbounded();

    std::thread::spawn(move || {
        let mut child = match Command::new("flow")
            .args(["install", "--porcelain"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // The installer's own errors, and curl's. Kept off the protocol
            // stream but not thrown away - a failed install without a reason
            // is the worst version of this screen.
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                let _ = tx.unbounded_send(Event::Failed(format!(
                    "Couldn't run flow install ({err}). Is it on your PATH?"
                )));
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Some(event) = parse(&line) else { continue };
                if tx.unbounded_send(event).is_err() {
                    return; // the window closed
                }
            }
        }

        // The reader ended, so the child has closed its stdout - either it
        // finished or it failed. Only the exit status says which.
        let status = child.wait();
        let ok = matches!(status, Ok(status) if status.success());

        if !ok {
            let mut reason = String::new();
            if let Some(mut stderr) = stderr {
                use std::io::Read;
                let _ = stderr.read_to_string(&mut reason);
            }
            // The first line, not the last. `flow install` returns an anyhow
            // error from `main`, so what reaches stderr is "Error: <what we
            // said>" followed by a "Caused by:" chain - and taking the end of
            // that put the innermost mechanical cause on screen ("The requested
            // URL returned error: 416") in place of the sentence written for
            // this exact situation.
            let reason = reason
                .trim()
                .lines()
                .next()
                .unwrap_or_default()
                .trim_start_matches("Error:")
                .trim()
                .to_string();
            let _ = tx.unbounded_send(Event::Failed(if reason.is_empty() {
                "Download interrupted.".into()
            } else {
                reason
            }));
        }
    });

    rx
}

/// One protocol line. An unparseable line is skipped rather than treated as a
/// failure: a newer daemon adding an event must not break an older window.
fn parse(line: &str) -> Option<Event> {
    let (word, rest) = match line.split_once(' ') {
        Some((word, rest)) => (word, rest),
        None => (line, ""),
    };
    Some(match word {
        "total" => Event::Total(rest.parse().ok()?),
        "progress" => Event::Progress(rest.parse().ok()?),
        "verifying" => Event::Verifying(rest.to_string()),
        "fetching" => Event::Fetching(rest.split_once(' ').map_or(rest, |(dest, _)| dest).into()),
        "installed" => Event::Installed,
        "finished" => Event::Finished,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// The model coming down, whether that is first run or a repair.
pub struct State {
    pub total: u64,
    pub done: u64,
    /// Where the ring is actually drawn, which chases `done` rather than
    /// matching it. Progress arrives in bursts of tens of megabytes; a ring
    /// that jumped between them would tick like a clock instead of filling.
    pub shown: f32,
    pub phase: Phase,
    /// Seconds this screen has been up. See `FLOOR`.
    pub elapsed: f32,
    /// 0 to 1 as the byte figure arrives, on its own clock.
    ///
    /// The line holds a non-breaking space until the installer reports a total,
    /// and until then there is no figure to show - so this cannot ride the
    /// intro's stagger, which by that point has usually finished. Without it
    /// "0 MB of 2.6 GB" appeared at full strength in a column that had already
    /// settled, which is the one thing on this screen that still popped.
    pub count_in: f32,
    /// True once `flow install` has been spawned. The intro plays first so
    /// hashing what is already on disk cannot hitch the fade.
    pub spawned: bool,
    /// True once anything has actually come down the wire. Sticky, so the
    /// caption does not fall back to "checking" between the last byte and the
    /// daemon starting.
    pub fetching: bool,
    /// Set after setup starts the daemon, so nothing issues that start twice.
    pub daemon_started: bool,
    /// True while systemd is starting Flow.
    pub starting_daemon: bool,
    /// Starting the daemon is separate from installing the model. Its error
    /// lives here so setup can offer the right retry instead of downloading
    /// again.
    pub start_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Between the window opening and the first line arriving.
    Starting,
    Verifying(String),
    Fetching(String),
    Done,
    Failed(String),
}

impl State {
    pub fn new() -> Self {
        Self {
            total: 0,
            done: 0,
            shown: 0.0,
            phase: Phase::Starting,
            elapsed: 0.0,
            count_in: 0.0,
            spawned: false,
            fetching: false,
            daemon_started: false,
            starting_daemon: false,
            start_error: None,
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Total(total) => self.total = total,
            // The ring is driven by bytes, so a file landing needs no state
            // of its own - the installer reports `progress` against the whole
            // install straight after every asset, verified ones included.
            Event::Installed => {}
            Event::Progress(done) => self.done = self.done.max(done),
            Event::Verifying(what) => self.phase = Phase::Verifying(what),
            Event::Fetching(what) => {
                self.fetching = true;
                self.phase = Phase::Fetching(what);
            }
            Event::Finished => {
                self.done = self.total;
                self.phase = Phase::Done;
            }
            Event::Failed(why) => self.phase = Phase::Failed(why),
        }
    }

    /// Move the drawn ring. Frame-rate independent, so it fills at the same
    /// speed whatever the compositor is doing, and forward only - a ring that
    /// can retreat is a ring that has been caught guessing.
    ///
    /// Two motions, because an install has two kinds of moment in it.
    ///
    /// When there is a gap to close, it is eased and then speed-limited. An
    /// exponential chase alone is at its fastest the instant the gap opens,
    /// which suits the small steps a live download arrives in and is wrong for
    /// the big ones: an install resuming onto a part file that is already three
    /// quarters written, or the whole recogniser reported in a single line
    /// after it hashes. Either moved most of the ring inside one frame, which
    /// reads as the ring having been redrawn rather than filled. `SPEED` is
    /// only a ceiling - fast enough that three quarters of the ring is crossed
    /// in well under a second, slow enough that the crossing is a fill you can
    /// watch. The ease takes over once the gap is small enough to close under
    /// the limit, so it lands rather than stopping.
    ///
    /// When there is no gap, it drifts - slowly, and only a little. The
    /// installer says nothing at all while it hashes a file already on disk,
    /// which on a resumed run is the opening seconds of the screen, and a ring
    /// stopped dead through them reads as broken. `CREEP` is a fraction of the
    /// catch-up rate and `LEAD` caps how far ahead of the last real figure it
    /// may get, so the drift can never reach the end on its own and the true
    /// figure always overtakes it.
    pub fn advance(&mut self, seconds: f32) {
        self.elapsed += seconds;
        if self.total > 0 {
            self.count_in = (self.count_in + seconds / COUNT_IN).min(1.0);
        }

        let target = self.fraction();
        let gap = target - self.shown;
        let step = if gap > 0.0 {
            (gap * (1.0 - (-seconds / CHASE).exp())).min(SPEED * seconds)
        } else if self.spawned && self.downloading() {
            (CREEP * seconds).min((target + LEAD - self.shown).max(0.0))
        } else {
            0.0
        };
        self.shown = (self.shown + step).clamp(0.0, 1.0);
    }

    fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.done as f32 / self.total as f32).clamp(0.0, 1.0)
    }

    /// True while anything is still moving, so the window only asks for frames
    /// when there is something to draw. The intro counts, even before the
    /// installer has been spawned.
    pub fn running(&self) -> bool {
        !matches!(self.phase, Phase::Failed(_))
            && (self.downloading()
                || !self.settled()
                || !self.intro_over()
                || (self.total > 0 && self.count_in < 1.0)
                || (self.shown - self.fraction()).abs() > 0.0005)
    }

    /// The overlay and every one of its contents have arrived, so the download
    /// may start. The last element's delay is part of it, or the frames stop
    /// with the button still half painted.
    pub fn intro_over(&self) -> bool {
        self.elapsed >= INTRO
    }

    /// Retry from an error already on this screen: the veil is up, so play
    /// no intro and start fetching on the next frame.
    pub fn skip_intro(&mut self) {
        self.elapsed = self.elapsed.max(INTRO);
    }

    /// The one line under the ring, which is the whole of what this screen
    /// says.
    pub fn caption(&self) -> String {
        match self.failed() {
            Some(why) => why.to_string(),
            None if self.starting_daemon => "Starting Flow…".to_string(),
            // The last thing on screen, and the reason the outro holds before
            // it runs. A repair that finds everything right is over in about a
            // second, and a second that ends in silence reads as a button that
            // did not work - so it ends on the answer instead.
            None if self.finished() => match self.fetching {
                true => "Flow is ready".to_string(),
                false => "Nothing to fix".to_string(),
            },
            // Hashing a missing file is microseconds, so on a first run this is
            // a state no frame ever paints. On a repair with the model already
            // on disk it is the whole screen - checking every file's length
            // and hashing what claims to be right - and calling that a
            // download was why it read as a fault.
            None if !self.fetching => "Checking files".to_string(),
            None => "Downloading the speech model".to_string(),
        }
    }

    /// Whether this screen has been up long enough to be allowed to finish.
    /// See `FLOOR`.
    pub fn settled(&self) -> bool {
        self.elapsed >= FLOOR
    }

    /// Whether the install is over, however it ended.
    pub fn downloading(&self) -> bool {
        !matches!(self.phase, Phase::Done | Phase::Failed(_))
    }

    /// Whether this run got what it came for and the screen may move on.
    pub fn finished(&self) -> bool {
        self.settled() && self.phase == Phase::Done && self.fraction() - self.shown < 0.01
    }

    pub fn failed(&self) -> Option<&str> {
        match (&self.start_error, &self.phase) {
            (Some(why), _) | (_, Phase::Failed(why)) => Some(why),
            _ => None,
        }
    }
}

/// The label for the screen's one control, or `None` to render nothing.
///
/// Only a failure offers anything. The model is not optional, so a download
/// in progress has nothing to decide against - there is no cancel to offer,
/// only a retry once something has actually gone wrong.
fn action_label(state: &State) -> Option<&'static str> {
    state.failed().map(|_| {
        if state.start_error.is_some() {
            "Start Flow"
        } else {
            "Try again"
        }
    })
}

/// How quickly the drawn ring settles onto the real figure once it is within
/// reach, as a time constant. This is what a resumed run mostly costs: the
/// speed limit crosses the bulk of the circle in half a second and then hands
/// over to the ease, whose tail is the rest. At a fifth of a second the whole
/// catch-up lands inside a second and a half and still settles rather than
/// stopping; much slower and the last third of the ring visibly crawls.
const CHASE: f32 = 0.22;

/// The most of the ring the drawn arc may cover in a second. A resumed run
/// opens with a fifth to three quarters of the circle to travel and this is
/// what decides how long that takes to watch - a shade over a second for the
/// whole circle, so a resume lands in about half of one. A live download moves
/// far slower than this, so the ceiling never touches it.
const SPEED: f32 = 1.2;

/// How the ring drifts while the installer hashes and reports nothing, and how
/// far ahead of the last real figure that drift may get. Slow, and a twentieth
/// of the circle: enough to see it is alive, not enough to be a claim.
const CREEP: f32 = 0.04;
const LEAD: f32 = 0.05;

/// How long the screen holds before it may hand over, in seconds.
///
/// A machine whose model is already on disk only hashes it, which can be over
/// in well under a second - and a screen that appears and vanishes inside that
/// reads as a glitch rather than as speed. A state needs roughly 300-600ms on
/// screen to register as a state at all.
const FLOOR: f32 = 0.8;

/// Full-screen veil first, then the contents over it.
const COVER: f32 = 0.28;
const RISE: f32 = 0.72;

/// How long the byte figure takes to arrive once there is a total to show.
/// Its own clock, because the total lands whenever the installer gets to it.
const COUNT_IN: f32 = 0.4;

/// How long each element waits behind the one above it. Small enough that the
/// column reads as one movement with a grain to it, rather than as four things
/// taking turns.
const STEP: f32 = 0.07;

/// Ring, caption, count, button - the four things that arrive in order.
const LIFTS: usize = 4;

/// When the last of them has landed.
const INTRO: f32 = COVER + STEP * (LIFTS - 1) as f32 + RISE;

/// How long the closing line stands still before the outro takes it away.
/// `fading` starts here as a negative, so the screen sits at full opacity for
/// exactly this long and then dissolves on the usual four beats.
pub const HOLD: f32 = 1.1;

/// The outro, in four beats rather than one dissolve. Slower than the intro on
/// purpose: arriving should get out of the way, finishing is the part worth
/// watching.
///
/// 1. `SHED` - the contents leave bottom-up: the button, the figure, the
///    caption. The ring stays. It is what the screen was about, and a ring that
///    dissolved alongside its own caption would be conceding rather than
///    finishing.
/// 2. `DRIFT` - the ring, now alone above an empty half-column, moves down into
///    the middle of the window it has to itself.
/// 3. `CLOSE` - it shuts to its own centre.
/// 4. `COVER` - the veil lifts on the console.
///
/// `SHED_END` is when the last of the contents has gone. The ring is index 0
/// and never sheds, so the last to leave is the caption at index 1.
const SHED: f32 = 0.42;
const DRIFT: f32 = 0.5;
const CLOSE: f32 = 0.5;
const SHED_END: f32 = SHED + STEP * (LIFTS - 2) as f32;

/// How far the ring travels in the `DRIFT` beat.
///
/// The page is one centred column, so removing height from below the ring is
/// not an option - it would reflow while the text is mid-fade. Growing a space
/// *above* it instead pushes the ring down by the full amount and the column's
/// own centring lifts it back by half, so the ring nets half of this. Set to
/// roughly what sits below it - gap, caption, gap, figure, gap, button - so the
/// ring lands about where the window's middle is.
const DRIFT_BY: f32 = 124.0;

/// How long the setup screen takes to dissolve into the console.
pub const FADE: f32 = SHED_END + DRIFT + CLOSE + COVER;

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

const RING: f32 = 96.0;

/// How wide the caption is allowed to get before it wraps. Around 60 characters
/// at 13px, which is the width a sentence is comfortable to read at and roughly
/// four times the ring - wide enough not to wrap the ordinary one-line captions,
/// narrow enough that an error reads as a paragraph rather than a banner.
const MEASURE: f32 = 380.0;

/// The whole of setup: a ring, a line, and nothing to press.
///
/// `fade` is 1 while setup owns the window and falls to 0 as the console
/// arrives underneath it. The veil covers first and the contents arrive over
/// it one behind the next; on the way out they leave in the opposite order and
/// the veil lifts last.
pub fn view(state: &State, fade: f32) -> Element<'_, Message> {
    let out_t = (1.0 - fade) * FADE;

    let veil_in = ease_out((state.elapsed / COVER).clamp(0.0, 1.0));
    let veil_out = ease_out(((out_t - SHED_END - DRIFT - CLOSE) / COVER).clamp(0.0, 1.0));
    let veil = veil_in * (1.0 - veil_out);

    // Each element arrives a beat behind the one above it and leaves a beat
    // ahead of it, so the column assembles downward and empties upward instead
    // of the whole block winking on and off as a single opacity.
    //
    // Opacity only, and deliberately: the page is centred, so anything that
    // animated a height or a padding would change the column's own size every
    // frame and drag its neighbours around with it.
    let arrive = |index: usize| {
        ease_out(((state.elapsed - COVER - STEP * index as f32) / RISE).clamp(0.0, 1.0))
    };
    let shed = |index: usize| {
        ease_out(((out_t - STEP * (LIFTS - 1 - index) as f32) / SHED).clamp(0.0, 1.0))
    };

    // The ring does not shed with the rest. Once they have gone it drifts into
    // the middle of the empty screen, and only then closes to its own centre -
    // the beat that says the thing finished rather than merely stopped being
    // shown.
    let ring_drift = ease_out(((out_t - SHED_END) / DRIFT).clamp(0.0, 1.0));
    let ring_close = ease_out(((out_t - SHED_END - DRIFT) / CLOSE).clamp(0.0, 1.0));
    let ring_lift = arrive(0) * (1.0 - ring_close);
    let caption_lift = arrive(1) * (1.0 - shed(1));
    let count_lift = arrive(2) * (1.0 - shed(2));
    let button_lift = arrive(3) * (1.0 - shed(3));

    let failed = state.failed();
    let caption = state.caption();

    let count = match (failed, state.total) {
        (None, total) if total > 0 => {
            let shown = (state.shown * total as f32) as u64;
            format!("{} of {}", ticking(shown), bytes(total))
        }
        // Keep the line's height from the first frame so the column does not
        // jump when the installer names a total.
        _ => "\u{00a0}".to_string(),
    };

    let mut page = column![
        // The `DRIFT` beat. See `DRIFT_BY`: growing this pushes the ring down
        // and the centred column lifts it back by half, so the ring moves and
        // the emptied space below it is what it moves into.
        Space::new().height(DRIFT_BY * ring_drift),
        Canvas::new(Ring {
            fraction: state.shown,
            fade: ring_lift,
            arrive: arrive(0),
            close: ring_close,
            failed: failed.is_some(),
        })
        .width(Length::Fixed(RING))
        .height(Length::Fixed(RING)),
        Space::new().height(26),
        // Bounded, because a caption is one line until it is an error and then
        // it is a sentence. Unbounded it ran the full width of the window,
        // which on a wide monitor is a single line of red spanning a metre of
        // glass under a 96px ring - and the column it belongs to is centred, so
        // it also dragged its own centre around as it grew. A measure narrower
        // than the ring's own column keeps it a paragraph.
        container(
            text(caption)
                .size(13)
                .width(Fill)
                .align_x(iced::Center)
                .color(emerge(
                    if failed.is_some() { ERR } else { MUTED },
                    caption_lift
                )),
        )
        .max_width(MEASURE),
        Space::new().height(8),
        // Two fades multiplied: the line's place in the intro, and the figure's
        // own arrival whenever the installer gets round to naming a total.
        text(count)
            .size(12)
            .color(emerge(FAINT, count_lift * ease_out(state.count_in))),
    ]
    .align_x(iced::alignment::Horizontal::Center);

    // Always in the tree, on both paths. Gating it on the fade finishing popped
    // a fully painted button into a centred column and shoved everything else
    // up.
    //
    // While downloading there is nothing to offer - the model is not
    // optional, so there is nothing to decide against - and an empty, faded
    // placeholder holds the row's height rather than leaving it out of the
    // tree, which is what stops the ring above it jumping the instant a
    // failure does give the user something to press.
    //
    // Outlined, not filled. The accent is green, and a saturated green button
    // is the brightest thing on a screen whose message is red - the eye landed
    // on the reassurance before the problem. Either way it is the only control
    // here, so it does not need a fill to be found.
    page = page.push(Space::new().height(22));
    page = page.push(match action_label(state) {
        Some(label) => crate::control::action_faded(label, false, button_lift, Message::BeginSetup),
        None => crate::control::action_faded("", false, 0.0, None::<Message>),
    });

    over(veil, page)
}

/// A page centred on the veil, at `veil` opacity: the ground covers the
/// console, the words sit in the middle.
fn over<'a>(veil: f32, page: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    stack![
        container(Space::new())
            .width(Fill)
            .height(Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color { a: veil, ..BG })),
                ..Default::default()
            }),
        container(page.into())
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center),
    ]
    .into()
}

/// The download, drawn as one closing circle.
///
/// Always determinate. It used to spin while the installer had not said how
/// much there was yet, which meant a resumed run spun through the verify pass
/// and then swapped a sweep at some arbitrary angle for an arc at twelve
/// o'clock - two different animations in the same ring, and the handover
/// between them read as a fault. An empty ring says the same thing the sweep
/// did and is the same shape the filling one is.
struct Ring {
    fraction: f32,
    fade: f32,
    /// 0 to 1 as the ring arrives. The canvas box is a fixed 96 square, so the
    /// one element on this screen that can be given real motion is this one:
    /// it eases open from just inside its final radius while it fades, and the
    /// column around it never moves a pixel.
    arrive: f32,
    /// 0 to 1 as the ring closes to its centre on the way out. Same fixed box,
    /// so this costs the column nothing either.
    close: f32,
    failed: bool,
}

impl canvas::Program<Message> for Ring {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let centre = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let full = bounds.width.min(bounds.height) / 2.0 - WIDTH;
        let radius = full * (OPEN + (1.0 - OPEN) * self.arrive) * (1.0 - self.close);
        // Below about a stroke width there is no ring left to draw, only a cap
        // sitting on the centre point - and a dot is not what the collapse was
        // meant to leave behind.
        if radius < WIDTH {
            return Vec::new();
        }
        let quiet = |colour: iced::Color| iced::Color {
            a: colour.a * self.fade,
            ..colour
        };

        let stroke = |colour: iced::Color| {
            Stroke::default()
                .with_color(quiet(colour))
                .with_width(WIDTH)
                .with_line_cap(canvas::LineCap::Round)
        };

        frame.stroke(&Path::circle(centre, radius), stroke(LINE));

        // Twelve o'clock is where a person starts reading a dial, and canvas
        // angles start at three.
        let top = -std::f32::consts::FRAC_PI_2;
        let sweep = if self.failed {
            std::f32::consts::TAU
        } else {
            std::f32::consts::TAU * self.fraction
        };

        // Nothing yet is drawn as nothing. A round cap on a zero-length arc
        // puts a dot at twelve o'clock before a single byte has arrived.
        if sweep > 0.0 {
            frame.stroke(
                &Path::new(|path| {
                    path.arc(canvas::path::Arc {
                        center: centre,
                        radius,
                        start_angle: Radians(top),
                        end_angle: Radians(top + sweep),
                    });
                }),
                stroke(if self.failed { ERR } else { ACCENT }),
            );
        }

        vec![frame.into_geometry()]
    }
}

/// The ring's stroke.
const WIDTH: f32 = 4.0;

/// The radius the ring opens from, as a share of its final one. Small enough
/// to be motion, large enough that it never reads as a different shape.
const OPEN: f32 = 0.88;

/// Bytes as a person reads them, in the decimal units a download is quoted in.
pub fn bytes(value: u64) -> String {
    match value {
        0 => String::new(),
        1..=999_999 => format!("{} KB", value / 1_000),
        1_000_000..=999_999_999 => format!("{} MB", value / 1_000_000),
        _ => format!("{:.1} GB", value as f64 / 1e9),
    }
}

/// The rising figure, in steps small enough to watch. `bytes` quotes the total
/// in the unit a download is advertised in, which above 1 GB is tenths - 100 MB
/// a tick, a hitch of its own. Megabytes, then hundredths of a gigabyte.
fn ticking(value: u64) -> String {
    match value {
        0 => "0 MB".into(),
        1..=999_999 => format!("{} KB", value / 1_000),
        1_000_000..=999_999_999 => format!("{} MB", value / 1_000_000),
        _ => format!("{:.2} GB", value as f64 / 1e9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled(state: State) -> State {
        State {
            elapsed: FLOOR,
            ..state
        }
    }

    fn speech() -> State {
        State::new()
    }

    fn catch_up(state: &mut State) {
        for _ in 0..5_000 {
            if state.fraction() - state.shown < 0.0005 && state.settled() {
                return;
            }
            state.advance(1.0 / 60.0);
        }
        panic!(
            "never arrived: shown {} target {}",
            state.shown,
            state.fraction()
        );
    }

    #[test]
    fn protocol_lines_become_events() {
        assert!(matches!(
            parse("total 2594569679"),
            Some(Event::Total(2_594_569_679))
        ));
        assert!(matches!(parse("progress 12"), Some(Event::Progress(12))));
        assert!(matches!(parse("finished"), Some(Event::Finished)));
        assert!(matches!(
            parse("installed nemotron/tokenizer.model"),
            Some(Event::Installed)
        ));
        // The dest is the first field; the size after it belongs to the ring's
        // total, not to the label.
        assert!(
            matches!(parse("fetching nemotron/tokenizer.model 406554"), Some(Event::Fetching(d)) if d == "nemotron/tokenizer.model")
        );
    }

    /// A line from a newer daemon must be ignored, not fatal.
    #[test]
    fn unknown_lines_are_skipped() {
        assert!(parse("something-new 4").is_none());
        assert!(parse("").is_none());
        assert!(parse("total not-a-number").is_none());
    }

    /// One model per run, so its own total is the whole story: no other
    /// asset's bytes counted into this ring.
    #[test]
    fn one_run_measures_the_whole_install() {
        let mut state = speech();
        state.apply(Event::Total(2_594_569_679));
        state.apply(Event::Progress(1_297_284_840));

        assert_eq!(state.total, 2_594_569_679);
        assert!((state.fraction() - 0.5).abs() < 0.001);
    }

    /// A verify-only run can be over in under a second, and a screen that
    /// appears and vanishes inside that reads as a glitch.
    #[test]
    fn setup_holds_before_it_hands_over() {
        let mut state = speech();
        state.apply(Event::Finished);
        assert!(!state.finished());
        // Still asking for frames, which is what carries it past the hold.
        assert!(state.running());

        catch_up(&mut state);
        assert!(state.finished());
    }

    /// There is no voluntary stop that turns a `Failed` line into a quiet
    /// finish instead - whatever the installer reports, it reports as a fault.
    #[test]
    fn a_reported_failure_is_always_a_fault() {
        let mut state = State {
            total: 100,
            done: 30,
            ..speech()
        };
        state.apply(Event::Failed("no route to host".into()));
        assert_eq!(state.phase, Phase::Failed("no route to host".into()));
        assert_eq!(state.failed(), Some("no route to host"));
    }

    /// The model is not optional, so there is nothing to decide against while
    /// it is still coming down - only a failure gives the user something to
    /// press.
    #[test]
    fn a_download_in_progress_offers_no_action() {
        let state = State {
            total: 100,
            done: 30,
            ..speech()
        };
        assert_eq!(action_label(&state), None);
    }

    #[test]
    fn a_failure_offers_a_way_to_retry() {
        let mut state = speech();
        state.apply(Event::Failed("no route to host".into()));
        assert_eq!(action_label(&state), Some("Try again"));
    }

    #[test]
    fn a_daemon_start_failure_offers_to_start_rather_than_retry_the_fetch() {
        let state = State {
            phase: Phase::Done,
            start_error: Some("systemd user session is unavailable".into()),
            ..speech()
        };
        assert_eq!(action_label(&state), Some("Start Flow"));
    }

    /// The ring approaches the real figure and settles, rather than snapping
    /// to it or overshooting past it.
    #[test]
    fn the_ring_chases_without_overshooting() {
        let mut state = State {
            total: 100,
            done: 100,
            phase: Phase::Done,
            ..speech()
        };
        catch_up(&mut state);
        assert!(state.shown <= 1.0, "overshot to {}", state.shown);
        assert!(state.shown > 0.99, "never arrived: {}", state.shown);
        assert!(!state.running());
    }

    /// A resumed run opens with most of the ring to travel. It has to be
    /// travelled - at a rate the eye can follow - rather than taken in a frame.
    #[test]
    fn a_resumed_run_travels_its_first_jump() {
        let mut state = State {
            total: 100,
            ..speech()
        };
        state.apply(Event::Progress(96));

        // No frame may take more than the speed limit allows, however wide the
        // gap it opened with.
        state.advance(1.0 / 60.0);
        assert!(
            state.shown <= SPEED / 60.0 + f32::EPSILON,
            "jolted to {}",
            state.shown
        );

        // Still travelling a fifth of a second in - this is a fill, not a cut.
        for _ in 0..11 {
            state.advance(1.0 / 60.0);
        }
        assert!(state.shown < 0.5, "raced to {}", state.shown);

        // And over inside two seconds, rather than crawling home.
        for _ in 0..108 {
            state.advance(1.0 / 60.0);
        }
        assert!(
            state.shown > 0.95,
            "still short at two seconds: {}",
            state.shown
        );
    }

    /// While the installer hashes in silence the ring drifts rather than
    /// sitting empty, and never far enough to be mistaken for real progress.
    #[test]
    fn the_ring_drifts_while_nothing_is_reported() {
        let mut state = State {
            total: 100,
            phase: Phase::Verifying("encoder".into()),
            spawned: true,
            ..speech()
        };
        for _ in 0..90 {
            state.advance(1.0 / 60.0);
        }
        assert!(state.shown > 0.0, "stayed empty");
        assert!(
            state.shown <= LEAD + f32::EPSILON,
            "wandered to {}",
            state.shown
        );
        assert!(state.running());
    }

    /// The intro is not over until the last of the four staggered elements has
    /// landed, or the frames stop with the button still half painted.
    #[test]
    fn the_download_waits_for_the_intro() {
        let state = speech();
        assert!(!state.intro_over());

        let mut mid = speech();
        mid.advance(COVER + RISE);
        assert!(
            !mid.intro_over(),
            "the trailing elements are still arriving"
        );

        let mut later = speech();
        later.advance(INTRO);
        assert!(later.intro_over());
        assert_eq!(later.shown, 0.0);
    }

    #[test]
    fn progress_never_goes_backwards() {
        let mut state = State {
            total: 100,
            ..speech()
        };
        state.apply(Event::Progress(60));
        // A part file is stat'd while curl writes it, so a short read after a
        // rename could otherwise pull the ring back.
        state.apply(Event::Progress(12));
        assert_eq!(state.done, 60);
    }

    /// Starting the daemon is a separate failure from downloading, and offers
    /// a different way out.
    #[test]
    fn daemon_start_failure_has_a_focused_recovery() {
        let state = settled(State {
            phase: Phase::Done,
            start_error: Some("systemd user session is unavailable".into()),
            ..speech()
        });
        assert_eq!(state.failed(), Some("systemd user session is unavailable"));
    }

    #[test]
    fn bytes_read_the_way_a_download_is_quoted() {
        assert_eq!(bytes(0), "");
        assert_eq!(bytes(670_619_803), "670 MB");
        assert_eq!(bytes(2_497_281_120), "2.5 GB");
        assert_eq!(ticking(0), "0 MB");
        assert_eq!(ticking(670_619_803), "670 MB");
        assert_eq!(ticking(1_850_000_000), "1.85 GB");
    }
}

#[cfg(test)]
mod closing_line {
    use super::*;

    fn done(fetching: bool) -> State {
        State {
            elapsed: FLOOR,
            phase: Phase::Done,
            fetching,
            ..State::new()
        }
    }

    #[test]
    fn a_repair_that_found_nothing_wrong_says_so() {
        // The whole point of the hold: this run takes about a second, and a
        // second that ends in silence is what got the button reported broken.
        assert_eq!(done(false).caption(), "Nothing to fix");
    }

    #[test]
    fn a_run_that_fetched_something_ends_on_the_install() {
        assert_eq!(done(true).caption(), "Flow is ready");
    }

    #[test]
    fn nothing_fetched_yet_is_a_check_rather_than_a_download() {
        let checking = State {
            phase: Phase::Verifying("nemotron/tokenizer.model".into()),
            ..State::new()
        };
        assert_eq!(checking.caption(), "Checking files");
    }

    #[test]
    fn a_failure_outranks_the_closing_line() {
        let broken = State {
            phase: Phase::Failed("no route to host".into()),
            fetching: true,
            ..done(true)
        };
        assert_eq!(broken.caption(), "no route to host");
    }
}
